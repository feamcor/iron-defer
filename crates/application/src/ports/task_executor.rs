//! Task execution port.
//!
//! Defines the contract for executing a claimed task. Concrete implementations
//! are wired up to the `TaskRegistry` and worker pool.

use async_trait::async_trait;
use iron_defer_domain::{TaskContext, TaskError, TaskRecord};

/// Execution port for tasks claimed from a queue.
///
/// Takes `&TaskRecord` (for `kind`, `payload`, `id`) and `&TaskContext`
/// (for `worker_id`, `attempt`) so the executor has full context for
/// dispatch and per-attempt tracing.
///
/// `#[automock]` generates `MockTaskExecutor` for application-layer unit tests.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TaskExecutor: Send + Sync + 'static {
    /// Execute a single claimed task with its execution context.
    async fn execute(&self, task: &TaskRecord, ctx: &TaskContext) -> Result<(), TaskError>;
}
