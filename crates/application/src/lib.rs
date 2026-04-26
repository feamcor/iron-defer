//! iron-defer application crate.
//!
//! Use-case orchestration and port traits. Depends only on the domain crate.
//! Per the architecture rule "no logic in lib.rs", this file only re-exports.

#![forbid(unsafe_code)]

pub mod config;
pub mod metrics;
pub mod observability;
pub mod ports;
pub mod registry;
pub mod services;

pub use crate::config::{
    AppConfig, DatabaseConfig, ObservabilityConfig, ProducerConfig, ServerConfig, WorkerConfig,
};
pub use crate::metrics::{
    Metrics, CLAIM_BACKOFF_SECONDS, CLAIM_BACKOFF_TOTAL, IDEMPOTENCY_KEYS_CLEANED_TOTAL,
    POOL_CONNECTIONS_ACTIVE, POOL_CONNECTIONS_IDLE, POOL_CONNECTIONS_TOTAL, SUSPEND_TIMEOUT_TOTAL,
    TASKS_PENDING, TASKS_RUNNING, TASKS_SUSPENDED_TOTAL, TASK_ATTEMPTS_TOTAL, TASK_DURATION_SECONDS,
    TASK_FAILURES_TOTAL, WORKER_POOL_UTILIZATION, ZOMBIE_RECOVERIES_TOTAL,
};
pub use crate::observability::emit_otel_state_transition;
pub use crate::ports::{RecoveryOutcome, TaskExecutor, TaskRepository, TransactionalTaskRepository};
pub use crate::registry::{TaskHandler, TaskRegistry};
pub use crate::services::{SchedulerService, SweeperService, WorkerService, drain_join_set};
