//! `OTel` metrics instruments and initialization.
//!
//! Architecture §D5.1 defines seven `iron_defer_*` instruments.
//! This module implements all of them plus three pool-connection observable
//! gauges (FR20). The [`Metrics`] struct holds synchronous instrument handles
//! that the worker and sweeper services record into directly; the observable
//! (callback-driven) gauges for `tasks_pending`, `tasks_running`, and pool
//! stats are registered separately via [`register_pool_gauges`].
//!
//! # Background refresh for async-queried gauges
//!
//! `tasks_pending` and `tasks_running` require a SQL `GROUP BY queue, status`
//! on each update. `OTel` observable-gauge callbacks run synchronously on
//! whichever thread invokes the reader — for Prometheus that is the axum
//! request worker thread, already inside the Tokio runtime. Calling
//! `Handle::block_on` from there panics with *"Cannot start a runtime from
//! within a runtime"*. Instead, a background Tokio task refreshes a shared
//! snapshot (`Arc<RwLock<TaskCountSnapshot>>`) every
//! [`DEFAULT_TASK_COUNT_REFRESH_INTERVAL`] (overridable for tests via the
//! `IRON_DEFER_TASK_COUNT_REFRESH_MS` env var — see
//! [`task_count_refresh_interval`]); the callback does only a read-lock +
//! iteration, with no async calls.
//!
//! # Dependency layering
//!
//! `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-prometheus`, and
//! `prometheus` live in this crate (infrastructure). The application crate
//! imports only the `opentelemetry` API crate for [`KeyValue`]. The api
//! crate imports `prometheus` for the `/metrics` handler's `TextEncoder`.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use iron_defer_application::{
    Metrics, ObservabilityConfig, CLAIM_BACKOFF_SECONDS, CLAIM_BACKOFF_TOTAL,
    IDEMPOTENCY_KEYS_CLEANED_TOTAL, POOL_CONNECTIONS_ACTIVE, POOL_CONNECTIONS_IDLE,
    POOL_CONNECTIONS_TOTAL, SUSPEND_TIMEOUT_TOTAL, TASKS_PENDING, TASKS_RUNNING,
    TASKS_SUSPENDED_TOTAL, TASK_ATTEMPTS_TOTAL, TASK_DURATION_SECONDS, TASK_FAILURES_TOTAL,
    WORKER_POOL_UTILIZATION, ZOMBIE_RECOVERIES_TOTAL,
};
use iron_defer_domain::TaskError;
use opentelemetry::KeyValue;
use opentelemetry::metrics::Meter;
use tokio_util::sync::CancellationToken;

/// Default interval between background refreshes of the task-count snapshot.
const DEFAULT_TASK_COUNT_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

const REFRESH_INTERVAL_ENV: &str = "IRON_DEFER_TASK_COUNT_REFRESH_MS";

fn parse_refresh_interval(raw: Option<&str>) -> Duration {
    match raw {
        Some(s) => match s.trim().parse::<u64>() {
            Ok(ms) if ms > 0 => Duration::from_millis(ms),
            _ => DEFAULT_TASK_COUNT_REFRESH_INTERVAL,
        },
        None => DEFAULT_TASK_COUNT_REFRESH_INTERVAL,
    }
}

fn task_count_refresh_interval() -> Duration {
    parse_refresh_interval(std::env::var(REFRESH_INTERVAL_ENV).ok().as_deref())
}

/// Per-(queue, status) count snapshot read by the gauge callbacks.
#[derive(Default, Debug)]
pub struct TaskCountSnapshot {
    pub pending: Vec<(String, u64)>,
    pub running: Vec<(String, u64)>,
}

#[must_use]
pub fn create_metrics(meter: &Meter) -> Metrics {
    let task_duration_seconds = meter
        .f64_histogram(TASK_DURATION_SECONDS)
        .with_description("Task execution duration in seconds")
        .with_unit("s")
        .build();

    let task_attempts_total = meter
        .u64_counter(TASK_ATTEMPTS_TOTAL)
        .with_description("Cumulative task attempt count")
        .build();

    let task_failures_total = meter
        .u64_counter(TASK_FAILURES_TOTAL)
        .with_description("Cumulative task failure count")
        .build();

    let zombie_recoveries_total = meter
        .u64_counter(ZOMBIE_RECOVERIES_TOTAL)
        .with_description("Tasks recovered by the sweeper")
        .build();

    let suspend_timeout_total = meter
        .u64_counter(SUSPEND_TIMEOUT_TOTAL)
        .with_description("Tasks auto-failed by the suspend watchdog due to timeout")
        .build();

    let tasks_suspended_total = meter
        .u64_counter(TASKS_SUSPENDED_TOTAL)
        .with_description("Tasks that transitioned to the Suspended state")
        .build();

    let worker_pool_utilization = meter
        .f64_gauge(WORKER_POOL_UTILIZATION)
        .with_description("Active/max workers ratio")
        .build();

    let claim_backoff_total = meter
        .u64_counter(CLAIM_BACKOFF_TOTAL)
        .with_description("Cumulative claim backoff events")
        .build();

    let claim_backoff_seconds = meter
        .f64_histogram(CLAIM_BACKOFF_SECONDS)
        .with_description("Claim backoff duration in seconds")
        .with_unit("s")
        .build();

    let idempotency_keys_cleaned_total = meter
        .u64_counter(IDEMPOTENCY_KEYS_CLEANED_TOTAL)
        .with_description("Idempotency keys cleaned by the sweeper")
        .build();

    Metrics {
        task_duration_seconds,
        task_attempts_total,
        task_failures_total,
        zombie_recoveries_total,
        suspend_timeout_total,
        tasks_suspended_total,
        worker_pool_utilization,
        claim_backoff_total,
        claim_backoff_seconds,
        idempotency_keys_cleaned_total,
        meter: meter.clone(),
    }
}

#[must_use]
pub fn register_pool_gauges(
    metrics: &Metrics,
    pool: &sqlx::PgPool,
    token: &CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let meter = &metrics.meter;

    let pool_for_total = pool.clone();
    let _total = meter
        .u64_observable_gauge(POOL_CONNECTIONS_TOTAL)
        .with_description("Total pool size")
        .with_callback(move |observer| {
            observer.observe(u64::from(pool_for_total.size()), &[]);
        })
        .build();

    let pool_for_idle = pool.clone();
    let _idle = meter
        .u64_observable_gauge(POOL_CONNECTIONS_IDLE)
        .with_description("Idle pool connections")
        .with_callback(move |observer| {
            #[allow(clippy::cast_possible_truncation)]
            observer.observe(pool_for_idle.num_idle() as u64, &[]);
        })
        .build();

    let pool_for_active = pool.clone();
    let _active = meter
        .u64_observable_gauge(POOL_CONNECTIONS_ACTIVE)
        .with_description("Active (in-use) pool connections")
        .with_callback(move |observer| {
            let total = u64::from(pool_for_active.size());
            #[allow(clippy::cast_possible_truncation)]
            let idle = pool_for_active.num_idle() as u64;
            observer.observe(total.saturating_sub(idle), &[]);
        })
        .build();

    let snapshot = Arc::new(RwLock::new(TaskCountSnapshot::default()));

    let refresh_pool = pool.clone();
    let refresh_snapshot = snapshot.clone();
    let refresh_token = token.clone();
    let refresh_handle = tokio::spawn(async move {
        refresh_task_counts_loop(refresh_pool, refresh_snapshot, refresh_token).await;
    });

    let pending_snapshot = snapshot.clone();
    let _pending = meter
        .u64_observable_gauge(TASKS_PENDING)
        .with_description("Current pending task count")
        .with_callback(move |observer| {
            let Ok(snap) = pending_snapshot.read() else {
                return;
            };
            for (queue, count) in &snap.pending {
                observer.observe(*count, &[KeyValue::new("queue", queue.clone())]);
            }
        })
        .build();

    let running_snapshot = snapshot.clone();
    let _running = meter
        .u64_observable_gauge(TASKS_RUNNING)
        .with_description("Current running task count")
        .with_callback(move |observer| {
            let Ok(snap) = running_snapshot.read() else {
                return;
            };
            for (queue, count) in &snap.running {
                observer.observe(*count, &[KeyValue::new("queue", queue.clone())]);
            }
        })
        .build();

    refresh_handle
}

async fn refresh_task_counts_loop(
    pool: sqlx::PgPool,
    snapshot: Arc<RwLock<TaskCountSnapshot>>,
    token: CancellationToken,
) {
    refresh_once(&pool, &snapshot).await;

    let interval = task_count_refresh_interval();
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;

    loop {
        tokio::select! {
            () = token.cancelled() => break,
            _ = ticker.tick() => {
                refresh_once(&pool, &snapshot).await;
            }
        }
    }
}

async fn refresh_once(pool: &sqlx::PgPool, snapshot: &Arc<RwLock<TaskCountSnapshot>>) {
    let rows: Result<Vec<(String, String, i64)>, sqlx::Error> = sqlx::query_as(
        "SELECT queue, status, count(*) FROM tasks WHERE status IN ('pending', 'running') GROUP BY queue, status",
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let mut pending = Vec::new();
            let mut running = Vec::new();
            for (queue, status, count) in rows {
                let count_u64 = u64::try_from(count).unwrap_or(0);
                match status.as_str() {
                    "pending" => pending.push((queue, count_u64)),
                    "running" => running.push((queue, count_u64)),
                    _ => {}
                }
            }
            if let Ok(mut guard) = snapshot.write() {
                *guard = TaskCountSnapshot { pending, running };
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to refresh task-count snapshot; gauges may be stale");
        }
    }
}

#[cfg(feature = "bin-init")]
pub fn init_metrics(
    config: &ObservabilityConfig,
) -> Result<
    (
        opentelemetry_sdk::metrics::SdkMeterProvider,
        prometheus::Registry,
    ),
    TaskError,
> {
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    let registry = prometheus::Registry::new();
    let prometheus_exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .map_err(|e| TaskError::Storage {
            source: Box::new(e),
        })?;

    let mut builder = SdkMeterProvider::builder().with_reader(prometheus_exporter);

    if !config.otlp_endpoint.is_empty() {
        use opentelemetry_otlp::WithExportConfig;

        let otlp_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(&config.otlp_endpoint)
            .build()
            .map_err(|e| TaskError::Storage {
                source: Box::new(e),
            })?;

        let periodic_reader = opentelemetry_sdk::metrics::PeriodicReader::builder(
            otlp_exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .build();

        builder = builder.with_reader(periodic_reader);
    }

    let provider = builder.build();
    Ok((provider, registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_metrics_constructs_all_instruments() {
        let meter = opentelemetry::global::meter("test");
        let metrics = create_metrics(&meter);
        metrics.task_attempts_total.add(1, &[]);
        metrics.task_failures_total.add(1, &[]);
        metrics.zombie_recoveries_total.add(1, &[]);
        metrics.task_duration_seconds.record(0.5, &[]);
        metrics.worker_pool_utilization.record(0.75, &[]);
        metrics.claim_backoff_total.add(1, &[]);
        metrics.claim_backoff_seconds.record(0.5, &[]);
        metrics.idempotency_keys_cleaned_total.add(1, &[]);
    }

    #[test]
    fn test_internal_parser_logic() {
        assert_eq!(
            parse_refresh_interval(None),
            DEFAULT_TASK_COUNT_REFRESH_INTERVAL
        );
        assert_eq!(
            parse_refresh_interval(Some("200")),
            Duration::from_millis(200)
        );
    }
}
