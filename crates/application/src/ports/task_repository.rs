//! Task persistence port.
//!
//! Object-safe so it can be injected as `Arc<dyn TaskRepository>`. Uses
//! `async-trait` because native `async fn` in traits is not yet object-safe
//! at MSRV 1.94.

use std::time::Duration;

use async_trait::async_trait;
use iron_defer_domain::{
    CancelResult, ListAuditLogResult, ListTasksFilter, ListTasksResult, QueueName,
    QueueStatistics, TaskError, TaskId, TaskKind, TaskRecord, WorkerId, WorkerStatus,
};

/// Persistence port for task records.
///
/// Adapters implementing this port (e.g. `PostgresTaskRepository` in the
/// infrastructure crate) translate their backend
/// errors into the domain `TaskError` enum at the boundary.
///
/// `#[automock]` generates `MockTaskRepository` for the
/// application-layer unit tests so the `SchedulerService` can be exercised
/// against mock expectations without a real database. See architecture C6.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TaskRepository: Send + Sync + 'static {
    /// Persist a task record. On success, returns the stored record (which
    /// may include backend-populated fields such as default timestamps).
    async fn save(&self, task: &TaskRecord) -> Result<TaskRecord, TaskError>;

    /// Persist a task with idempotency-key conflict detection.
    ///
    /// If a non-terminal task with the same `(queue, idempotency_key)` already
    /// exists, returns the existing record and `created = false`. Otherwise
    /// inserts the new task and returns `created = true`.
    async fn save_idempotent(&self, task: &TaskRecord) -> Result<(TaskRecord, bool), TaskError>;

    /// Clean up expired idempotency keys on terminal tasks.
    ///
    /// Sets `idempotency_key = NULL` and `idempotency_expires_at = NULL` on
    /// tasks where the retention window has elapsed. Returns the count of
    /// cleaned keys.
    async fn cleanup_expired_idempotency_keys(&self) -> Result<u64, TaskError>;

    /// Look up a task by its identifier.
    async fn find_by_id(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError>;

    /// List all tasks belonging to a given queue.
    async fn list_by_queue(&self, queue: &QueueName) -> Result<Vec<TaskRecord>, TaskError>;

    /// Atomically claim the next eligible task from a queue using `SKIP LOCKED`.
    ///
    /// Returns `Ok(None)` when no pending tasks are available. The claimed task
    /// transitions to `Running` with `attempts` incremented and a lease set to
    /// `now() + lease_duration`.
    #[cfg_attr(test, mockall::concretize)]
    async fn claim_next(
        &self,
        queue: &QueueName,
        worker_id: WorkerId,
        lease_duration: Duration,
        region: Option<&str>,
    ) -> Result<Option<TaskRecord>, TaskError>;

    /// Transition a running task to `Completed`.
    ///
    /// Returns an error if the task is not in `Running` status.
    async fn complete(&self, task_id: TaskId) -> Result<TaskRecord, TaskError>;

    /// Record a task failure with retry-vs-terminal logic.
    ///
    /// If `attempts < max_attempts`, the task transitions back to `Pending` with
    /// exponential backoff applied to `scheduled_at`. Otherwise it transitions to
    /// `Failed`. Backoff formula: `min(base_delay_secs * 2^(attempts-1), max_delay_secs)`.
    async fn fail(
        &self,
        task_id: TaskId,
        error_message: &str,
        base_delay_secs: f64,
        max_delay_secs: f64,
    ) -> Result<TaskRecord, TaskError>;

    /// Recover zombie tasks — running tasks whose lease has expired.
    ///
    /// Retryable tasks (`attempts < max_attempts`) are reset to `Pending` with
    /// `claimed_by` and `claimed_until` cleared and `scheduled_at` set to now.
    /// Exhausted tasks (`attempts >= max_attempts`) transition to `Failed`.
    ///
    /// Returns `(TaskId, QueueName, TaskKind, Option<String>, RecoveryOutcome)`
    /// tuples for all recovered and failed tasks so the caller has context
    /// for metrics/logging/observability.
    async fn recover_zombie_tasks(
        &self,
    ) -> Result<Vec<(TaskId, QueueName, TaskKind, Option<String>, RecoveryOutcome)>, TaskError>;

    /// List tasks with optional filters and pagination.
    async fn list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError>;

    /// Return per-queue statistics: pending count, running count, active workers.
    /// If `by_region` is true, returns stats grouped by (queue, region).
    async fn queue_statistics(&self, by_region: bool) -> Result<Vec<QueueStatistics>, TaskError>;

    /// Cancel a pending task. Returns the cancellation outcome:
    /// - `Cancelled(record)` if the task was pending and is now cancelled.
    /// - `NotFound` if no task with the given ID exists.
    /// - `NotCancellable { current_status }` if the task is not in `Pending` status.
    async fn cancel(&self, task_id: TaskId) -> Result<CancelResult, TaskError>;

    /// Return per-worker status: `worker_id`, queue, tasks in flight.
    async fn worker_status(&self) -> Result<Vec<WorkerStatus>, TaskError>;

    /// Release all leases held by a specific worker, returning tasks to `Pending`.
    ///
    /// Used during graceful shutdown when the drain timeout expires and in-flight
    /// tasks have not completed. Clears `claimed_by` and `claimed_until`, sets
    /// `scheduled_at = now()` for immediate re-availability.
    ///
    /// Returns the `(TaskId, Option<String>)` (with trace_id) of all released tasks.
    async fn release_leases_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<(TaskId, Option<String>)>, TaskError>;

    /// Release the lease for a specific task, returning it to `Pending`.
    ///
    /// Used when a worker claims a task but cannot dispatch it (e.g. shutdown
    /// detected after claim).
    ///
    /// Returns the trace_id if present.
    async fn release_lease_for_task(&self, task_id: TaskId) -> Result<Option<String>, TaskError>;

    /// Query audit log entries for a given task, ordered by timestamp ascending.
    ///
    /// Returns an empty result when audit logging is disabled.
    async fn audit_log(
        &self,
        task_id: TaskId,
        limit: i64,
        offset: i64,
    ) -> Result<ListAuditLogResult, TaskError>;

    /// Transition a running task to `Suspended`.
    ///
    /// Clears `claimed_by` and `claimed_until` to release the logical claim,
    /// sets `suspended_at = now()`. Returns an error if the task is not in
    /// `Running` status.
    async fn suspend(&self, task_id: TaskId) -> Result<TaskRecord, TaskError>;

    /// Resume a suspended task by transitioning it back to `Pending`.
    ///
    /// Stores the optional signal payload and sets `scheduled_at = now()` for
    /// immediate re-claiming. The `WHERE status = 'suspended'` guard ensures
    /// concurrent signals are safely serialized — only one succeeds.
    async fn signal(
        &self,
        task_id: TaskId,
        payload: Option<serde_json::Value>,
    ) -> Result<TaskRecord, TaskError>;

    /// Auto-fail suspended tasks that exceeded the suspend timeout.
    ///
    /// Returns the `(TaskId, QueueName)` of each expired task for metrics/logging.
    async fn expire_suspended_tasks(
        &self,
        suspend_timeout: Duration,
    ) -> Result<Vec<(TaskId, QueueName)>, TaskError>;
}

/// Transactional persistence port for task records.
///
/// Separated from [`TaskRepository`] because `sqlx::Transaction` parameters
/// are incompatible with `mockall::automock` (lifetime + mutable reference).
/// Integration tests exercise these methods directly; unit tests use the
/// mockable `TaskRepository` trait for non-transactional paths.
#[async_trait]
pub trait TransactionalTaskRepository: Send + Sync + 'static {
    async fn save_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        record: &TaskRecord,
    ) -> Result<TaskRecord, TaskError>;

    async fn save_idempotent_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        record: &TaskRecord,
    ) -> Result<(TaskRecord, bool), TaskError>;
}

/// Outcome of a zombie task recovery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Task was reset to Pending status for retry.
    Recovered,
    /// Task exhausted retries and was transitioned to Failed status.
    Failed,
}
