//! Story 3.3 — `OTel` metric signal compliance (ACs 3-5).
//!
//! Split from `otel_compliance_test.rs` for maintainability. Each test
//! runs in its own binary and gets its own `OnceCell<TestDb>` pool, so
//! no Mutex serializer is needed.
//!
//! - `histogram_records_completed_duration` (AC 3 / P2-INT-001)
//! - `gauges_match_db_state` (AC 4 / P2-INT-002)
//! - `worker_pool_utilization_reports_ratio` (AC 5 / P2-INT-003)
//! - `pool_connection_gauges_are_emitted` (P2-INT-010)

mod common;

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use iron_defer::{IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

use common::otel::{await_all_terminal, build_harness, find_sample, scrape_samples, with_worker};

// ---------------------------------------------------------------------------
// Task fixture.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OtelSleepTask {
    sleep_ms: u64,
}

impl Task for OtelSleepTask {
    const KIND: &'static str = "otel_sleep_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AC 3 / P2-INT-001.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn histogram_records_completed_duration() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let harness = build_harness();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<OtelSleepTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .shutdown_timeout(Duration::from_secs(1))
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    engine
        .enqueue(&queue, OtelSleepTask { sleep_ms: 50 })
        .await
        .expect("enqueue task");

    with_worker(engine.clone(), |engine, _token| {
        let queue = queue.clone();
        async move {
            assert!(
                await_all_terminal(&engine, &queue, 30, Duration::from_millis(200)).await,
                "task did not complete within the 6 s budget (see stderr for stuck-task diagnostic)"
            );
        }
    })
    .await;

    let samples = scrape_samples(&harness.registry);

    let count = find_sample(
        &samples,
        "iron_defer_task_duration_seconds_seconds_count",
        &[
            ("queue", queue.as_str()),
            ("kind", "otel_sleep_task"),
            ("status", "completed"),
        ],
    )
    .expect("histogram _count sample for completed otel_sleep_task");
    assert!(
        count.value >= 1.0,
        "expected at least one completed duration sample, got {}",
        count.value
    );

    let sum = find_sample(
        &samples,
        "iron_defer_task_duration_seconds_seconds_sum",
        &[
            ("queue", queue.as_str()),
            ("kind", "otel_sleep_task"),
            ("status", "completed"),
        ],
    )
    .expect("histogram _sum sample for completed otel_sleep_task");
    assert!(
        sum.value >= 0.04,
        "expected _sum >= 40 ms (the sleep_ms lower bound), got {}",
        sum.value
    );
    assert!(
        sum.value <= 10.0,
        "expected _sum <= 10 s (sanity upper bound; got {} — task may be stuck)",
        sum.value
    );

    let le_5_bucket = samples.iter().any(|s| {
        s.metric == "iron_defer_task_duration_seconds_seconds_bucket"
            && s.labels.get("queue").map(String::as_str) == Some(queue.as_str())
            && s.labels.get("kind").map(String::as_str) == Some("otel_sleep_task")
            && s.labels.get("le").map(String::as_str) == Some("5")
    });
    assert!(
        le_5_bucket,
        "expected a `_bucket{{le=\"5\"}}` line for a 50 ms sleep — SDK default bucket boundaries missing"
    );

    harness.provider.shutdown().expect("provider shutdown");
}

// ---------------------------------------------------------------------------
// AC 4 / P2-INT-002.
// ---------------------------------------------------------------------------

// OnceLock ensures the env var is set exactly once per binary lifetime.
// True per-test scoping is impossible: env vars are process-global and
// `set_var` races with multi-thread tokio runtimes. The OnceLock pattern
// sets the var before any engine reads it (during gauge background-task
// setup), which is safe because OnceLock serializes the single write.
// TODO: make gauge refresh interval a builder parameter instead of env var
// (Epic 7+) — that removes the env-var dependency entirely.
static REFRESH_INTERVAL_ENV_SET: OnceLock<()> = OnceLock::new();

fn set_fast_refresh_interval() {
    REFRESH_INTERVAL_ENV_SET.get_or_init(|| {
        // SAFETY: called exactly once (OnceLock) before the engine's gauge
        // background loop reads the var; no concurrent reader yet exists.
        unsafe {
            std::env::set_var("IRON_DEFER_TASK_COUNT_REFRESH_MS", "200");
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn gauges_match_db_state() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    set_fast_refresh_interval();

    let queue = common::unique_queue();
    let harness = build_harness();

    let worker_config = WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<OtelSleepTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    for _ in 0..3 {
        engine
            .enqueue(&queue, OtelSleepTask { sleep_ms: 1500 })
            .await
            .expect("enqueue task");
    }

    with_worker(engine.clone(), |_engine, _token| {
        let pool = pool.clone();
        let queue = queue.clone();
        let harness_registry = harness.registry.clone();
        async move {
            let mut last_mismatch: Option<String> = None;
            let queue_str = queue.as_str();

            for _attempt in 0..8 {
                tokio::time::sleep(Duration::from_millis(200)).await;

                let samples = scrape_samples(&harness_registry);
                let pending = find_sample(
                    &samples,
                    "iron_defer_tasks_pending",
                    &[("queue", queue_str)],
                )
                .map(|s| s.value);
                let running = find_sample(
                    &samples,
                    "iron_defer_tasks_running",
                    &[("queue", queue_str)],
                )
                .map(|s| s.value);

                let db_rows: Vec<(String, i64)> = sqlx::query_as(
                    "SELECT status::text, count(*) FROM tasks WHERE queue = $1 GROUP BY status",
                )
                .bind(queue_str)
                .fetch_all(&pool.clone())
                .await
                .expect("db cross-check query");

                let mut db_pending: i64 = 0;
                let mut db_running: i64 = 0;
                for (status, count) in &db_rows {
                    match status.as_str() {
                        "pending" => db_pending = *count,
                        "running" => db_running = *count,
                        _ => {}
                    }
                }

                if pending == Some(1.0)
                    && running == Some(2.0)
                    && db_pending == 1
                    && db_running == 2
                {
                    return;
                }

                last_mismatch = Some(format!(
                    "scrape(pending={pending:?}, running={running:?}) \
                     db(pending={db_pending}, running={db_running})"
                ));
            }
            panic!(
                "gauges_match_db_state never converged within 1.6 s; last={}",
                last_mismatch.as_deref().unwrap_or("<none>")
            );
        }
    })
    .await;

    harness.provider.shutdown().expect("provider shutdown");
}

// ---------------------------------------------------------------------------
// AC 5 / P2-INT-003.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn worker_pool_utilization_reports_ratio() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let harness = build_harness();

    let worker_config = WorkerConfig {
        concurrency: 4,
        poll_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<OtelSleepTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    for _ in 0..2 {
        engine
            .enqueue(&queue, OtelSleepTask { sleep_ms: 600 })
            .await
            .expect("enqueue task");
    }

    with_worker(engine.clone(), |engine, _token| {
        let queue = queue.clone();
        let harness_registry = harness.registry.clone();
        async move {
            let queue_str = queue.as_str();
            let mut saw_active = false;
            for _ in 0..7 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let samples = scrape_samples(&harness_registry);
                if let Some(s) = find_sample(
                    &samples,
                    "iron_defer_worker_pool_utilization",
                    &[("queue", queue_str)],
                ) && s.value > 0.0
                    && s.value <= 0.5 + 1e-9
                {
                    saw_active = true;
                    break;
                }
            }
            assert!(
                saw_active,
                "never observed a non-zero iron_defer_worker_pool_utilization in the 1 s window"
            );

            assert!(
                await_all_terminal(&engine, &queue, 30, Duration::from_millis(200)).await,
                "tasks did not all complete within 6 s (see stderr for stuck-task diagnostic)"
            );
        }
    })
    .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let samples = scrape_samples(&harness.registry);
    let final_util = find_sample(
        &samples,
        "iron_defer_worker_pool_utilization",
        &[("queue", queue.as_str())],
    )
    .expect("final utilization sample");
    assert!(
        (final_util.value - 0.0).abs() < 1e-9,
        "expected post-completion utilization = 0.0, got {}",
        final_util.value
    );

    harness.provider.shutdown().expect("provider shutdown");
}

// ---------------------------------------------------------------------------
// P2-INT-010 — Pool connection gauges emitted.
// ---------------------------------------------------------------------------

/// Verify that `register_pool_gauges` emits `pool_connections_total`,
/// `pool_connections_idle`, and `pool_connections_active` via the
/// Prometheus exporter. The invariant `total = idle + active` must hold.
#[tokio::test(flavor = "multi_thread")]
async fn pool_connection_gauges_are_emitted() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let harness = build_harness();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<OtelSleepTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .shutdown_timeout(Duration::from_secs(1))
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    engine
        .enqueue(&queue, OtelSleepTask { sleep_ms: 50 })
        .await
        .expect("enqueue task");

    with_worker(engine.clone(), |engine, _token| {
        let queue = queue.clone();
        let harness_registry = harness.registry.clone();
        async move {
            assert!(
                await_all_terminal(&engine, &queue, 30, Duration::from_millis(200)).await,
                "task did not complete within the 6 s budget (see stderr for stuck-task diagnostic)"
            );

            let samples = scrape_samples(&harness_registry);

            let total = find_sample(&samples, "iron_defer_pool_connections_total", &[])
                .expect("pool_connections_total gauge missing");
            assert!(
                total.value >= 1.0,
                "expected pool_connections_total >= 1, got {}",
                total.value
            );

            let idle = find_sample(&samples, "iron_defer_pool_connections_idle", &[])
                .expect("pool_connections_idle gauge missing");

            let active = find_sample(&samples, "iron_defer_pool_connections_active", &[])
                .expect("pool_connections_active gauge missing");

            let sum = idle.value + active.value;
            assert!(
                (sum - total.value).abs() < 1e-9,
                "invariant violated: idle({}) + active({}) != total({})",
                idle.value,
                active.value,
                total.value
            );
        }
    })
    .await;

    harness.provider.shutdown().expect("provider shutdown");
}
