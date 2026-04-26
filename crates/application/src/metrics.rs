//! `OTel` metric instrument handles.
//!
//! This struct lives in the application crate because worker and sweeper
//! services need it, and the application crate must not depend on the
//! infrastructure crate (hexagonal layering). The struct holds only
//! `opentelemetry` API types — no SDK types.

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};

// Architecture §Naming Patterns: OTel instrument constants in SCREAMING_SNAKE_CASE;
// metric string names in `iron_defer_snake_case`.
pub const TASKS_PENDING: &str = "iron_defer_tasks_pending";
pub const TASKS_RUNNING: &str = "iron_defer_tasks_running";
pub const TASK_DURATION_SECONDS: &str = "iron_defer_task_duration_seconds";
pub const TASK_ATTEMPTS_TOTAL: &str = "iron_defer_task_attempts_total";
pub const TASK_FAILURES_TOTAL: &str = "iron_defer_task_failures_total";
pub const ZOMBIE_RECOVERIES_TOTAL: &str = "iron_defer_zombie_recoveries_total";
pub const SUSPEND_TIMEOUT_TOTAL: &str = "iron_defer_suspend_timeout_total";
pub const TASKS_SUSPENDED_TOTAL: &str = "iron_defer_tasks_suspended_total";
pub const WORKER_POOL_UTILIZATION: &str = "iron_defer_worker_pool_utilization";
pub const CLAIM_BACKOFF_TOTAL: &str = "iron_defer_claim_backoff_total";
pub const CLAIM_BACKOFF_SECONDS: &str = "iron_defer_claim_backoff_seconds";
pub const IDEMPOTENCY_KEYS_CLEANED_TOTAL: &str = "iron_defer_idempotency_keys_cleaned_total";
pub const POOL_CONNECTIONS_ACTIVE: &str = "iron_defer_pool_connections_active";
pub const POOL_CONNECTIONS_IDLE: &str = "iron_defer_pool_connections_idle";
pub const POOL_CONNECTIONS_TOTAL: &str = "iron_defer_pool_connections_total";

/// Synchronous `OTel` metric instrument handles.
///
/// Created via `create_metrics` in the infrastructure crate (or
/// `iron_defer::create_metrics` from the public API). The struct is
/// `Clone + Send + Sync` (all `OTel` instrument handles are cheap clones).
#[derive(Clone, Debug)]
pub struct Metrics {
    /// Histogram: task execution duration in seconds.
    /// Labels: `queue`, `kind`, `status`.
    pub task_duration_seconds: Histogram<f64>,

    /// Counter: cumulative attempt count.
    /// Labels: `queue`, `kind`.
    pub task_attempts_total: Counter<u64>,

    /// Counter: cumulative failure count (FR44: includes terminal failures).
    /// Labels: `queue`, `kind`.
    pub task_failures_total: Counter<u64>,

    /// Counter: tasks recovered by the sweeper.
    /// Labels: `queue`.
    pub zombie_recoveries_total: Counter<u64>,

    /// Counter: tasks auto-failed by the suspend watchdog due to timeout.
    /// Labels: `queue`.
    pub suspend_timeout_total: Counter<u64>,

    /// Counter: tasks that transitioned to the Suspended state.
    /// Labels: `queue`, `kind`.
    pub tasks_suspended_total: Counter<u64>,

    /// Gauge: active/max workers ratio.
    /// Labels: `queue`.
    pub worker_pool_utilization: Gauge<f64>,

    /// Counter: claim backoff events.
    /// Labels: `queue`, `saturation`.
    pub claim_backoff_total: Counter<u64>,

    /// Histogram: claim backoff duration in seconds.
    /// Labels: `queue`.
    pub claim_backoff_seconds: Histogram<f64>,

    /// Counter: idempotency keys cleaned by the sweeper.
    /// No labels.
    pub idempotency_keys_cleaned_total: Counter<u64>,

    /// Handle for the `Meter` that built these instruments.
    ///
    /// Carried so `register_pool_gauges` registers observable gauges on the
    /// same provider backing this `Metrics` — not on the global no-op
    /// provider. Callers must build `Metrics` via `create_metrics(&meter)`
    /// and then pass the struct to the builder; `IronDefer::start` reads
    /// this handle instead of reaching for `global::meter(...)`.
    pub meter: Meter,
}
