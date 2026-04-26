//! Scheduler service — typed enqueue / find / list over `TaskRepository`.
//!
//! Architecture references:
//! - §Process Patterns (Tracing instrumentation): every public async method
//!   gets `#[instrument(skip(self), fields(...), err)]`. `payload` is NEVER
//!   in `fields(...)` (FR38).
//! - §C6: `mockall::automock` on the `TaskRepository` port enables the unit
//!   tests below to exercise the service against mock expectations without
//!   a real database.
//!
//! `SchedulerService` is the application-layer facade over the
//! infrastructure adapter. The `IronDefer` engine in `crates/api/src/lib.rs`
//! holds one of these and delegates the public `enqueue` / `find` / `list`
//! methods to it.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use iron_defer_domain::{
    AttemptCount, CancelResult, ListTasksFilter, ListTasksResult, MaxAttempts, Priority,
    QueueName, QueueStatistics, TaskError, TaskId, TaskKind, TaskRecord, TaskStatus,
    WorkerStatus,
};
use tracing::instrument;

use crate::ports::{TaskRepository, TransactionalTaskRepository};

/// Default `max_attempts` for newly enqueued tasks.
const DEFAULT_MAX_ATTEMPTS: MaxAttempts = MaxAttempts::DEFAULT;

/// Default `priority` for newly enqueued tasks. Higher = picked sooner.
const DEFAULT_PRIORITY: Priority = Priority::DEFAULT;

/// Application-layer scheduler facade.
pub struct SchedulerService {
    repo: Arc<dyn TaskRepository>,
    tx_repo: Option<Arc<dyn TransactionalTaskRepository>>,
}

impl SchedulerService {
    /// Construct a scheduler over the given repository port.
    #[must_use]
    pub fn new(repo: Arc<dyn TaskRepository>) -> Self {
        Self {
            repo,
            tx_repo: None,
        }
    }

    /// Attach a transactional repository for `enqueue_in_tx` support.
    #[must_use]
    pub fn with_tx_repo(mut self, tx_repo: Arc<dyn TransactionalTaskRepository>) -> Self {
        self.tx_repo = Some(tx_repo);
        self
    }

    /// Enqueue a task with the given payload.
    pub async fn enqueue(
        &self,
        queue: &QueueName,
        kind: &'static str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
    ) -> Result<TaskRecord, TaskError> {
        self.enqueue_raw(queue, kind, payload, scheduled_at, None, None, None, None).await
    }

    /// Enqueue a task inside a caller-provided transaction.
    #[instrument(skip(self, tx, payload), fields(queue = %queue, kind = %kind, region), err)]
    pub async fn enqueue_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        queue: &QueueName,
        kind: &'static str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        region: Option<&str>,
    ) -> Result<TaskRecord, TaskError> {
        use iron_defer_domain::PayloadErrorKind;
        let tx_repo = self.tx_repo.as_ref().ok_or_else(|| TaskError::Storage {
            source: "transactional repository not configured".into(),
        })?;

        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }

        let now = Utc::now();
        let scheduled = scheduled_at.unwrap_or(now);

        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(queue.clone())
            .kind(
                TaskKind::try_from(kind).map_err(|_| TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: format!("Task kind {:?} must be non-empty", kind),
                    },
                })?,
            )
            .payload(Arc::new(payload))
            .status(TaskStatus::Pending)
            .priority(DEFAULT_PRIORITY)
            .attempts(AttemptCount::ZERO)
            .max_attempts(DEFAULT_MAX_ATTEMPTS)
            .scheduled_at(scheduled)
            .created_at(now)
            .updated_at(now)
            .maybe_region(region.map(str::to_owned))
            .build();
        record.validate_invariants();

        tx_repo.save_in_tx(tx, &record).await
    }

    /// Enqueue a task with idempotency key inside a caller-provided transaction.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, tx, payload), fields(queue = %queue, kind = %kind, idempotency_key = %idempotency_key, region), err)]
    pub async fn enqueue_in_tx_idempotent(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        queue: &QueueName,
        kind: &'static str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        idempotency_key: &str,
        retention: Duration,
        region: Option<&str>,
    ) -> Result<(TaskRecord, bool), TaskError> {
        use iron_defer_domain::PayloadErrorKind;
        let tx_repo = self.tx_repo.as_ref().ok_or_else(|| TaskError::Storage {
            source: "transactional repository not configured".into(),
        })?;

        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }

        let now = Utc::now();
        let scheduled = scheduled_at.unwrap_or(now);
        let retention_delta = chrono::Duration::from_std(retention).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid idempotency retention duration: {e}"),
            },
        })?;
        let expires_at = now.checked_add_signed(retention_delta).ok_or_else(|| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: "idempotency retention duration causes overflow".to_owned(),
            },
        })?;

        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(queue.clone())
            .kind(
                TaskKind::try_from(kind).map_err(|_| TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: format!("Task kind {:?} must be non-empty", kind),
                    },
                })?,
            )
            .payload(Arc::new(payload))
            .status(TaskStatus::Pending)
            .priority(DEFAULT_PRIORITY)
            .attempts(AttemptCount::ZERO)
            .max_attempts(DEFAULT_MAX_ATTEMPTS)
            .scheduled_at(scheduled)
            .created_at(now)
            .updated_at(now)
            .maybe_idempotency_key(Some(idempotency_key.to_owned()))
            .maybe_idempotency_expires_at(Some(expires_at))
            .maybe_region(region.map(str::to_owned))
            .build();
        record.validate_invariants();

        tx_repo.save_idempotent_in_tx(tx, &record).await
    }

    /// Look up a task by id.
    #[instrument(skip(self), fields(task_id = %id), err)]
    pub async fn find(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError> {
        self.repo.find_by_id(id).await
    }

    /// Enqueue a task using runtime-typed `kind` and raw JSON payload.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, payload), fields(queue = %queue, kind = %kind, trace_id, region), err)]
    pub async fn enqueue_raw(
        &self,
        queue: &QueueName,
        kind: &str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        priority: Option<i16>,
        max_attempts: Option<i32>,
        trace_id: Option<&str>,
        region: Option<&str>,
    ) -> Result<TaskRecord, TaskError> {
        use iron_defer_domain::PayloadErrorKind;

        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }

        let now = Utc::now();
        let scheduled = scheduled_at.unwrap_or(now);

        let prio = priority.map_or(DEFAULT_PRIORITY, Priority::new);
        let max = if let Some(v) = max_attempts {
            MaxAttempts::new(v).map_err(|e| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("invalid max_attempts: {e}"),
                },
            })?
        } else {
            DEFAULT_MAX_ATTEMPTS
        };

        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(queue.clone())
            .kind(
                TaskKind::try_from(kind).map_err(|_| TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "task kind must not be empty".to_owned(),
                    },
                })?,
            )
            .payload(Arc::new(payload))
            .status(TaskStatus::Pending)
            .priority(prio)
            .attempts(AttemptCount::ZERO)
            .max_attempts(max)
            .scheduled_at(scheduled)
            .created_at(now)
            .updated_at(now)
            .maybe_trace_id(trace_id.map(str::to_owned))
            .maybe_region(region.map(str::to_owned))
            .build();
        record.validate_invariants();

        self.repo.save(&record).await
    }

    /// Enqueue a task with idempotency key support.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, payload), fields(queue = %queue, kind = %kind, idempotency_key = %idempotency_key, region), err)]
    pub async fn enqueue_idempotent(
        &self,
        queue: &QueueName,
        kind: &'static str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        idempotency_key: &str,
        retention: Duration,
        region: Option<&str>,
    ) -> Result<(TaskRecord, bool), TaskError> {
        self.enqueue_raw_idempotent(
            queue,
            kind,
            payload,
            scheduled_at,
            None,
            None,
            idempotency_key,
            retention,
            None,
            region,
        ).await
    }

    /// Enqueue a task with runtime-typed kind and idempotency key support.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, payload), fields(queue = %queue, kind = %kind, idempotency_key = %idempotency_key, region), err)]
    pub async fn enqueue_raw_idempotent(
        &self,
        queue: &QueueName,
        kind: &str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        priority: Option<i16>,
        max_attempts: Option<i32>,
        idempotency_key: &str,
        retention: Duration,
        trace_id: Option<&str>,
        region: Option<&str>,
    ) -> Result<(TaskRecord, bool), TaskError> {
        use iron_defer_domain::PayloadErrorKind;

        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }

        let now = Utc::now();
        let scheduled = scheduled_at.unwrap_or(now);
        let retention_delta = chrono::Duration::from_std(retention).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid idempotency retention duration: {e}"),
            },
        })?;
        let expires_at = now.checked_add_signed(retention_delta).ok_or_else(|| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: "idempotency retention duration causes overflow".to_owned(),
            },
        })?;

        let prio = priority.map_or(DEFAULT_PRIORITY, Priority::new);
        let max = if let Some(v) = max_attempts {
            MaxAttempts::new(v).map_err(|e| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("invalid max_attempts: {e}"),
                },
            })?
        } else {
            DEFAULT_MAX_ATTEMPTS
        };

        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(queue.clone())
            .kind(
                TaskKind::try_from(kind).map_err(|_| TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "task kind must not be empty".to_owned(),
                    },
                })?,
            )
            .payload(Arc::new(payload))
            .status(TaskStatus::Pending)
            .priority(prio)
            .attempts(AttemptCount::ZERO)
            .max_attempts(max)
            .scheduled_at(scheduled)
            .created_at(now)
            .updated_at(now)
            .maybe_idempotency_key(Some(idempotency_key.to_owned()))
            .maybe_idempotency_expires_at(Some(expires_at))
            .maybe_trace_id(trace_id.map(str::to_owned))
            .maybe_region(region.map(str::to_owned))
            .build();
        record.validate_invariants();

        self.repo.save_idempotent(&record).await
    }

    /// Cancel a pending task.
    #[instrument(skip(self), fields(task_id = %id), err)]
    pub async fn cancel(&self, id: TaskId) -> Result<CancelResult, TaskError> {
        self.repo.cancel(id).await
    }

    /// List all tasks in a queue.
    #[instrument(skip(self), fields(queue = %queue), err)]
    pub async fn list(&self, queue: &QueueName) -> Result<Vec<TaskRecord>, TaskError> {
        self.repo.list_by_queue(queue).await
    }

    /// List tasks with optional filters and pagination.
    #[instrument(skip(self, filter), err)]
    pub async fn list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError> {
        self.repo.list_tasks(filter).await
    }

    /// Return per-queue statistics.
    #[instrument(skip(self), err)]
    pub async fn queue_statistics(&self, by_region: bool) -> Result<Vec<QueueStatistics>, TaskError> {
        self.repo.queue_statistics(by_region).await
    }

    /// Return per-worker status.
    #[instrument(skip(self), err)]
    pub async fn worker_status(&self) -> Result<Vec<WorkerStatus>, TaskError> {
        self.repo.worker_status().await
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    pub async fn audit_log(&self, task_id: TaskId, limit: i64, offset: i64) -> Result<iron_defer_domain::ListAuditLogResult, TaskError> {
        self.repo.audit_log(task_id, limit, offset).await
    }

    #[instrument(skip(self, payload), fields(task_id = %task_id), err)]
    pub async fn signal(
        &self,
        task_id: TaskId,
        payload: Option<serde_json::Value>,
    ) -> Result<TaskRecord, TaskError> {
        if let Some(ref p) = payload {
            let bytes = serde_json::to_vec(p).map_err(|e| TaskError::InvalidPayload {
                kind: iron_defer_domain::PayloadErrorKind::Serialization {
                    message: format!("failed to serialize signal payload: {e}"),
                },
            })?;
            if bytes.len() > iron_defer_domain::SIGNAL_PAYLOAD_MAX_BYTES {
                return Err(TaskError::InvalidPayload {
                    kind: iron_defer_domain::PayloadErrorKind::Validation {
                        message: format!(
                            "signal payload size {} exceeds maximum {} bytes",
                            bytes.len(),
                            iron_defer_domain::SIGNAL_PAYLOAD_MAX_BYTES
                        ),
                    },
                });
            }
        }
        self.repo.signal(task_id, payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::task_repository::MockTaskRepository;
    use mockall::predicate::*;

    fn sample_queue() -> QueueName {
        QueueName::try_from("test-queue").expect("valid queue")
    }

    fn synthetic_record(id: TaskId, queue: QueueName, kind: &str) -> TaskRecord {
        let now = Utc::now();
        TaskRecord::builder()
            .id(id)
            .queue(queue)
            .kind(TaskKind::try_from(kind).expect("test kind must be non-empty"))
            .payload(Arc::new(serde_json::json!({"echoed": true})))
            .status(TaskStatus::Pending)
            .priority(DEFAULT_PRIORITY)
            .attempts(AttemptCount::ZERO)
            .max_attempts(DEFAULT_MAX_ATTEMPTS)
            .scheduled_at(now)
            .created_at(now)
            .updated_at(now)
            .build()
    }

    #[tokio::test]
    async fn enqueue_calls_repo_save_with_constructed_record() {
        let queue = sample_queue();
        let queue_for_assert = queue.clone();

        let mut mock = MockTaskRepository::new();
        mock.expect_save()
            .once()
            .withf(move |task: &TaskRecord| {
                *task.kind() == "echo"
                    && *task.queue() == queue_for_assert
                    && task.status() == TaskStatus::Pending
                    && task.attempts() == AttemptCount::ZERO
                    && task.max_attempts() == DEFAULT_MAX_ATTEMPTS
                    && task.priority() == DEFAULT_PRIORITY
                    && *task.payload() == serde_json::json!({"data": 42})
            })
            .returning(|task| Ok(task.clone()));

        let scheduler = SchedulerService::new(Arc::new(mock));
        let result = scheduler
            .enqueue(&queue, "echo", serde_json::json!({"data": 42}), None)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn find_delegates_to_repo() {
        let id = TaskId::new();
        let synthetic = synthetic_record(id, sample_queue(), "echo");
        let synthetic_for_return = synthetic.clone();

        let mut mock = MockTaskRepository::new();
        mock.expect_find_by_id()
            .with(eq(id))
            .once()
            .returning(move |_| Ok(Some(synthetic_for_return.clone())));

        let scheduler = SchedulerService::new(Arc::new(mock));
        let found = scheduler.find(id).await.expect("find").expect("present");
        assert_eq!(found, synthetic);
    }
}
