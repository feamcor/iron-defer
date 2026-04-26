//! Story 3.3 AC 6 / P2-INT-004 — counter increment compliance.
//!
//! Split from `otel_compliance_test.rs` for maintainability.
//! `task_attempts_total` + `task_failures_total` increment on both
//! the retry and terminal branches (Story 3.2 AC 4 table, FR44).

mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig};
use serde::{Deserialize, Serialize};

use common::otel::{build_harness, find_sample, scrape_samples, with_worker};

// ---------------------------------------------------------------------------
// Task fixture — always fails.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OtelFlakyTask {}

impl Task for OtelFlakyTask {
    const KIND: &'static str = "otel_flaky_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Err(TaskError::ExecutionFailed {
            kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                source: "synthetic".into(),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// AC 6 / P2-INT-004.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn counters_increment_on_retry_and_terminal() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let harness = build_harness();

    let worker_config = WorkerConfig {
        concurrency: 1,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(1),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<OtelFlakyTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    let flaky = OtelFlakyTask {};
    let record = engine
        .enqueue_raw(
            &queue,
            OtelFlakyTask::KIND,
            serde_json::to_value(&flaky).expect("serialize"),
            None,
            None,
            Some(2),
            None,
            None,
        )
        .await
        .expect("enqueue_raw flaky task");

    with_worker(engine.clone(), |engine, _token| {
        let record_id = record.id();
        async move {
            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Some(r) = engine.find(record_id).await.expect("find task")
                    && matches!(r.status(), TaskStatus::Failed)
                {
                    return;
                }
            }
            panic!("flaky task never reached TaskStatus::Failed in 10 s");
        }
    })
    .await;

    let samples = scrape_samples(&harness.registry);

    let attempts = find_sample(
        &samples,
        "iron_defer_task_attempts_total_total",
        &[("queue", queue.as_str()), ("kind", "otel_flaky_task")],
    )
    .expect("task_attempts_total sample");
    assert!(
        (attempts.value - 2.0).abs() < 1e-9,
        "expected task_attempts_total = 2 for retry + terminal, got {}",
        attempts.value
    );

    let failures = find_sample(
        &samples,
        "iron_defer_task_failures_total_total",
        &[("queue", queue.as_str()), ("kind", "otel_flaky_task")],
    )
    .expect("task_failures_total sample");
    assert!(
        (failures.value - 2.0).abs() < 1e-9,
        "expected task_failures_total = 2 (retry + terminal), got {}",
        failures.value
    );

    let duration_count = find_sample(
        &samples,
        "iron_defer_task_duration_seconds_seconds_count",
        &[
            ("queue", queue.as_str()),
            ("kind", "otel_flaky_task"),
            ("status", "failed"),
        ],
    )
    .expect("task_duration_seconds_count with status=failed sample");
    assert!(
        (duration_count.value - 2.0).abs() < 1e-9,
        "expected failed-status histogram count = 2, got {}",
        duration_count.value
    );

    harness.provider.shutdown().expect("provider shutdown");
}
