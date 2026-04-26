//! iron-defer public library façade and embedded entry point.
//!
//! NOTE: this is the **sole** crate where logic in `lib.rs` is permitted —
//! see Architecture §Structure Patterns ("No logic in lib.rs" rule scope).
//!
//! # Overview
//!
//! ```ignore
//! use iron_defer::{IronDefer, Task, TaskContext, TaskError};
//! use serde::{Serialize, Deserialize};
//! use sqlx::PgPool;
//!
//! #[derive(Serialize, Deserialize)]
//! struct EmailTask { to: String, subject: String }
//!
//! impl Task for EmailTask {
//!     const KIND: &'static str = "email";
//!     async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
//!         // ... send email ...
//!         Ok(())
//!     }
//! }
//!
//! # async fn example(pool: PgPool) -> Result<(), TaskError> {
//! let engine = IronDefer::builder()
//!     .pool(pool)
//!     .register::<EmailTask>()
//!     .build()
//!     .await?;
//!
//! let task = EmailTask { to: "user@example.com".into(), subject: "Hi".into() };
//! engine.enqueue("default", task).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture invariants
//!
//! - The builder accepts a caller-provided `sqlx::PgPool` and never spawns
//!   its own Tokio runtime (Architecture §Enforcement Guidelines).
//! - `crates/api/src/lib.rs` is the SOLE construction site for
//!   `iron_defer_application::TaskRegistry` (Architecture §Process Patterns — TaskRegistry ownership).
//! - `sqlx::PgPool`, `&'static sqlx::migrate::Migrator` (via `migrator()`),
//!   and `sqlx::Transaction<'_, Postgres>` (via `enqueue_in_tx()`) are the
//!   only `sqlx` types that may cross the public API boundary
//!   (Architecture §Architectural Boundaries — Public library API boundary).
//!   Both `PgPool` and `Transaction` are caller-provided — iron-defer never
//!   constructs them.

#![forbid(unsafe_code)]

pub mod cli;
pub mod config;
pub mod http;

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Datelike, Utc};
use iron_defer_application::{SchedulerService, SweeperService, WorkerService};
use iron_defer_infrastructure::PostgresTaskRepository;
use sqlx::PgPool;
use tracing::instrument;

pub mod shutdown;

// Public re-exports — the iron-defer library API surface.
//
// Forbidden re-exports per Architecture §Architectural Boundaries: any
// `sqlx::*` type other than `PgPool` (input), `&'static Migrator`
// (output of `IronDefer::migrator`), and `Transaction<'_, Postgres>`
// (input to `IronDefer::enqueue_in_tx`); `axum::*`; `reqwest::*`;
// `iron_defer_infrastructure::PostgresTaskRepository`;
// `iron_defer_infrastructure::PostgresAdapterError`. Adding any of these
// to the re-export list is a story-killing bug.
pub use iron_defer_application::{
    DatabaseConfig, Metrics, TaskHandler, TaskRegistry, WorkerConfig,
};
pub use iron_defer_domain::{
    CancelResult, ExecutionErrorKind, ListTasksFilter, ListTasksResult, PayloadErrorKind,
    QueueName, QueueStatistics, Task, TaskContext, TaskError, TaskId, TaskRecord, TaskStatus,
    WorkerStatus,
};
pub use iron_defer_infrastructure::create_metrics;
pub use tokio_util::sync::CancellationToken;

// ----------------------------------------------------------------------------
// TaskHandlerAdapter — bridges `impl Task` to `Arc<dyn TaskHandler>`.
// ----------------------------------------------------------------------------

/// Generic adapter that turns a concrete `T: Task` into a type-erased
/// `TaskHandler`.
///
/// Architecture §C4 specifies this exact pattern. The adapter
/// holds zero state — it only carries the `T` type parameter so the
/// `execute` method can deserialize the payload into the right concrete
/// type before calling `T::execute(ctx)`.
struct TaskHandlerAdapter<T: Task>(PhantomData<T>);

impl<T: Task> TaskHandler for TaskHandlerAdapter<T> {
    fn kind(&self) -> &'static str {
        T::KIND
    }

    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
        Box::pin(async move {
            // Deserialize via the by-reference `Deserializer` impl on
            // `&serde_json::Value` so we avoid cloning the entire JSON tree
            // on the per-task hot path. Architecture §C4 calls out
            // explicit allocation control as the reason this trait does
            // NOT use `#[async_trait]`; honoring that intent means avoiding
            // hidden payload clones too.
            let task: T = T::deserialize(payload).map_err(|e| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Deserialization {
                    message: e.to_string(),
                },
            })?;
            task.execute(ctx).await
        })
    }
}

// ----------------------------------------------------------------------------
// IronDefer — embedded library engine.
// ----------------------------------------------------------------------------

/// Embedded `iron-defer` engine.
///
/// Construct via [`IronDefer::builder`]. The engine holds the application-
/// layer `SchedulerService`, an `Arc<TaskRegistry>` of registered task
/// handlers, and a clone of the caller-provided `PgPool`.
///
/// The engine exposes producer APIs (`enqueue*`), worker/sweeper lifecycle
/// APIs (`start`), task-management APIs (`find`, `list`, `cancel`, `signal`),
/// and infrastructure accessors (`pool`, `registry`, `migrator`).
pub struct IronDefer {
    scheduler: SchedulerService,
    registry: Arc<TaskRegistry>,
    pool: PgPool,
    worker_config: WorkerConfig,
    producer_config: iron_defer_application::ProducerConfig,
    queue: QueueName,
    /// `OTel` metric instrument handles for the worker and sweeper.
    metrics: Option<iron_defer_application::Metrics>,
    /// Prometheus registry for the `/metrics` scrape endpoint (FR18).
    /// `None` when metrics are not configured (embedded mode without `OTel`).
    pub(crate) prometheus_registry: Option<prometheus::Registry>,
    readiness_timeout: std::time::Duration,
    started: AtomicBool,
    audit_log: bool,
    unlogged_tables: bool,
}

impl IronDefer {
    fn validate_region_authorization(&self, region: Option<&str>) -> Result<(), TaskError> {
        if let Some(r) = region {
            validate_region(r)?;
            if !self.producer_config.allowed_regions.is_empty()
                && !self
                    .producer_config
                    .allowed_regions
                    .iter()
                    .any(|allowed| allowed == r)
            {
                return Err(TaskError::InvalidPayload {
                    kind: iron_defer_domain::PayloadErrorKind::Validation {
                        message: format!(
                            "unauthorized region '{r}'; allowed regions: {:?}",
                            self.producer_config.allowed_regions
                        ),
                    },
                });
            }
        }
        Ok(())
    }

    /// Begin constructing an engine.
    #[must_use]
    pub fn builder() -> IronDeferBuilder {
        IronDeferBuilder::default()
    }

    /// Embedded migration set.
    ///
    /// Returned for callers who manage migrations externally (e.g. inside
    /// a larger application transaction). The builder runs migrations
    /// automatically by default; pass `.skip_migrations(true)` to opt out
    /// and call `IronDefer::migrator().run(&pool)` from your own code.
    ///
    /// Architecture §C3 explicitly authorizes exposing the
    /// `&'static sqlx::migrate::Migrator` across the public API boundary.
    #[must_use]
    pub fn migrator() -> &'static sqlx::migrate::Migrator {
        &iron_defer_infrastructure::MIGRATOR
    }

    /// Repair a failed migration.
    ///
    /// If a migration failed partway, sqlx leaves the schema in a "dirty"
    /// state. This method calls [`sqlx::migrate::Migrator::repair`] to
    /// clear the dirty flag and allow migrations to be re-run.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Migration`] if the repair operation fails.
    pub async fn repair_migrations(pool: &PgPool) -> Result<(), TaskError> {
        iron_defer_infrastructure::repair_migrations(pool)
            .await
            .map_err(|e| TaskError::Migration {
                source: Box::new(e),
            })
    }

    /// Borrow the underlying connection pool. Useful for callers that
    /// need to issue ad-hoc queries against the same database without
    /// allocating a second pool.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Borrow the registered task-handler registry.
    #[must_use]
    pub fn registry(&self) -> &Arc<TaskRegistry> {
        &self.registry
    }

    #[must_use]
    pub fn readiness_timeout(&self) -> std::time::Duration {
        self.readiness_timeout
    }

    #[must_use]
    pub fn is_unlogged_tables(&self) -> bool {
        self.unlogged_tables
    }

    /// Enqueue a task for asynchronous execution. The task is persisted
    /// to `PostgreSQL` with status `Pending`; workers pick it up on a later
    /// poll cycle.
    ///
    /// `scheduled_at` defaults to "now" — for explicit scheduling, use
    /// [`IronDefer::enqueue_at`].
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if the queue name fails
    /// validation, no handler is registered for `T::KIND`, or the task
    /// cannot be serialized to JSON.
    /// Returns `TaskError::Storage` if the database `INSERT` fails.
    #[instrument(skip(self, task), fields(queue = %queue, kind = %T::KIND), err)]
    pub async fn enqueue<T: Task>(&self, queue: &str, task: T) -> Result<TaskRecord, TaskError> {
        self.enqueue_inner::<T>(queue, task, None).await
    }

    /// Enqueue a task with an explicit `scheduled_at`. The worker pool
    /// will not pick the task up before this timestamp.
    ///
    /// # Errors
    ///
    /// See [`IronDefer::enqueue`].
    #[instrument(
        skip(self, task),
        fields(queue = %queue, kind = %T::KIND, scheduled_at = %scheduled_at),
        err
    )]
    pub async fn enqueue_at<T: Task>(
        &self,
        queue: &str,
        task: T,
        scheduled_at: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        self.enqueue_inner::<T>(queue, task, Some(scheduled_at))
            .await
    }

    /// Enqueue a task with a region label for geographic pinning.
    ///
    /// Tasks enqueued with a region are only claimed by workers configured
    /// with the same region (or claimed by regional workers that also pick up
    /// unpinned tasks). Workers with no region configured will skip pinned tasks.
    #[instrument(skip(self, task), fields(queue = %queue, kind = %T::KIND, region = %region), err)]
    pub async fn enqueue_with_region<T: Task>(
        &self,
        queue: &str,
        task: T,
        region: &str,
    ) -> Result<TaskRecord, TaskError> {
        self.validate_region_authorization(Some(region))?;

        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "no handler registered for kind {:?} — call .register::<{}>() before .build()",
                        T::KIND,
                        std::any::type_name::<T>()
                    ),
                },
            });
        }

        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;
        let record = self
            .enqueue_raw(
                queue,
                T::KIND,
                payload,
                None,
                None,
                None,
                None,
                Some(region),
            )
            .await?;

        Ok(record)
    }

    async fn enqueue_inner<T: Task>(
        &self,
        queue: &str,
        task: T,
        scheduled_at: Option<DateTime<Utc>>,
    ) -> Result<TaskRecord, TaskError> {
        // Fail fast when no handler is registered for `T::KIND` so
        // registration mistakes surface at enqueue time.
        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "no handler registered for kind {:?} — call .register::<{}>() before .build()",
                        T::KIND,
                        std::any::type_name::<T>()
                    ),
                },
            });
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        if let Some(ref dt) = scheduled_at {
            validate_scheduled_at(dt)?;
        }
        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;
        let record = self
            .scheduler
            .enqueue(&queue_name, T::KIND, payload, scheduled_at)
            .await?;

        // Emit `task_enqueued` at the API façade where `log_payload` is
        // available without changing scheduler-layer signatures.
        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok(record)
    }

    /// Enqueue a task with an idempotency key. If a non-terminal task with the
    /// same key+queue already exists, returns the existing record and `created=false`.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if validation fails.
    /// Returns `TaskError::Storage` if the database operation fails.
    #[instrument(skip(self, task), fields(queue = %queue, kind = %T::KIND, idempotency_key = %idempotency_key), err)]
    pub async fn enqueue_idempotent<T: Task>(
        &self,
        queue: &str,
        task: T,
        idempotency_key: &str,
    ) -> Result<(TaskRecord, bool), TaskError> {
        if idempotency_key.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "idempotency key must not be empty".to_owned(),
                },
            });
        }
        if idempotency_key.len() > 250 {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "idempotency key length {} exceeds maximum of 250 characters",
                        idempotency_key.len()
                    ),
                },
            });
        }
        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "no handler registered for kind {:?} — call .register::<{}>() before .build()",
                        T::KIND,
                        std::any::type_name::<T>()
                    ),
                },
            });
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;

        let retention = self.worker_config.idempotency_key_retention;
        let (record, created) = self
            .scheduler
            .enqueue_idempotent(
                &queue_name,
                T::KIND,
                payload,
                None,
                idempotency_key,
                retention,
                None,
            )
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok((record, created))
    }

    /// Enqueue a task with both an idempotency key and a region label.
    #[instrument(skip(self, task), fields(queue = %queue, kind = %T::KIND, idempotency_key = %idempotency_key, region = %region), err)]
    pub async fn enqueue_idempotent_with_region<T: Task>(
        &self,
        queue: &str,
        task: T,
        idempotency_key: &str,
        region: &str,
    ) -> Result<(TaskRecord, bool), TaskError> {
        validate_region(region)?;
        if idempotency_key.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "idempotency key must not be empty".to_owned(),
                },
            });
        }
        if idempotency_key.len() > 250 {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "idempotency key length {} exceeds maximum of 250 characters",
                        idempotency_key.len()
                    ),
                },
            });
        }
        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("no handler registered for kind {:?}", T::KIND),
                },
            });
        }
        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;

        let retention = self.worker_config.idempotency_key_retention;
        let (record, created) = self
            .scheduler
            .enqueue_idempotent(
                &queue_name,
                T::KIND,
                payload,
                None,
                idempotency_key,
                retention,
                Some(region),
            )
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok((record, created))
    }

    /// Enqueue a task inside a caller-provided database transaction.
    ///
    /// The caller owns the transaction — iron-defer executes a single INSERT
    /// on it and never calls `BEGIN`, `COMMIT`, or `ROLLBACK`. If the caller
    /// commits, the task becomes visible to workers on the next poll cycle.
    /// If the caller rolls back, the task never existed.
    ///
    /// This is an embedded-library-only feature; the REST API and CLI do not
    /// support transactional enqueue.
    #[instrument(skip(self, tx, task), fields(queue = %queue, kind = %T::KIND, region), err)]
    pub async fn enqueue_in_tx<T: Task>(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        queue: &str,
        task: T,
        region: Option<&str>,
    ) -> Result<TaskRecord, TaskError> {
        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }
        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "no handler registered for kind {:?} — call .register::<{}>() before .build()",
                        T::KIND,
                        std::any::type_name::<T>()
                    ),
                },
            });
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;

        let record = self
            .scheduler
            .enqueue_in_tx(tx, &queue_name, T::KIND, payload, None, region)
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok(record)
    }

    /// Enqueue a task with an idempotency key inside a caller-provided
    /// database transaction. Combines transactional atomicity (G2) with
    /// duplicate suppression (G1).
    ///
    /// The caller owns the transaction. Deduplication is scoped to the
    /// transaction — uncommitted rows from other transactions are invisible
    /// via Postgres MVCC READ COMMITTED.
    #[instrument(skip(self, tx, task), fields(queue = %queue, kind = %T::KIND, idempotency_key = %idempotency_key, region), err)]
    pub async fn enqueue_in_tx_idempotent<T: Task>(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        queue: &str,
        task: T,
        idempotency_key: &str,
        region: Option<&str>,
    ) -> Result<(TaskRecord, bool), TaskError> {
        if idempotency_key.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "idempotency key must not be empty".to_owned(),
                },
            });
        }
        if idempotency_key.len() > 250 {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "idempotency key length {} exceeds maximum of 250 characters",
                        idempotency_key.len()
                    ),
                },
            });
        }
        if let Some(r) = region {
            if r.is_empty() {
                return Err(TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: "region label must not be empty".to_owned(),
                    },
                });
            }
        }
        if self.registry.get(T::KIND).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "no handler registered for kind {:?} — call .register::<{}>() before .build()",
                        T::KIND,
                        std::any::type_name::<T>()
                    ),
                },
            });
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        let payload = serde_json::to_value(&task).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Serialization {
                message: format!("task payload serialization failed: {e}"),
            },
        })?;

        let retention = self.worker_config.idempotency_key_retention;
        let (record, created) = self
            .scheduler
            .enqueue_in_tx_idempotent(
                tx,
                &queue_name,
                T::KIND,
                payload,
                None,
                idempotency_key,
                retention,
                region,
            )
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok((record, created))
    }

    /// Look up a task by id and deserialize its payload.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::NotFound` if no task with the given id exists.
    /// Returns `TaskError::InvalidPayload` if the payload cannot be deserialized or if the task kind mismatch.
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), fields(task_id = %id, kind = %T::KIND), err)]
    pub async fn get<T: Task>(&self, id: TaskId) -> Result<T, TaskError> {
        let record = self
            .find(id)
            .await?
            .ok_or_else(|| TaskError::NotFound { id })?;

        if record.kind().as_str() != T::KIND {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "task kind mismatch: expected {:?}, got {:?}",
                        T::KIND,
                        record.kind().as_str()
                    ),
                },
            });
        }

        let payload: T =
            T::deserialize(record.payload()).map_err(|e| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Deserialization {
                    message: e.to_string(),
                },
            })?;

        Ok(payload)
    }

    /// Look up a task by id. Returns `Ok(None)` if no task with the given
    /// id exists.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), fields(task_id = %id), err)]
    pub async fn find(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError> {
        self.scheduler.find(id).await
    }

    /// List all tasks in the given queue.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if the queue name is invalid
    /// or `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), fields(queue = %queue), err)]
    pub async fn list(&self, queue: &str) -> Result<Vec<TaskRecord>, TaskError> {
        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;
        self.scheduler.list(&queue_name).await
    }

    /// Cancel a pending task. Returns the cancellation outcome.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database operation fails.
    #[instrument(skip(self), fields(task_id = %id), err)]
    pub async fn cancel(&self, id: TaskId) -> Result<iron_defer_domain::CancelResult, TaskError> {
        let result = self.scheduler.cancel(id).await?;

        if let iron_defer_domain::CancelResult::Cancelled(ref record) = result {
            tracing::info!(
                event = "task_cancelled",
                task_id = %record.id(),
                queue = %record.queue(),
                kind = %record.kind(),
                "task cancelled by operator"
            );
        }

        Ok(result)
    }

    /// Resume a suspended task with an optional signal payload.
    ///
    /// Delegates to `TaskRepository::signal()` which atomically transitions
    /// the task from `Suspended` to `Pending` with the signal data stored.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::NotFound` if the task doesn't exist.
    /// Returns `TaskError::NotInExpectedState` if the task is not `Suspended`.
    /// Returns `TaskError::Storage` if the database operation fails.
    #[instrument(skip(self, payload), fields(task_id = %task_id), err)]
    pub async fn signal(
        &self,
        task_id: TaskId,
        payload: Option<serde_json::Value>,
    ) -> Result<TaskRecord, TaskError> {
        self.scheduler.signal(task_id, payload).await
    }

    /// List tasks with optional filters and pagination.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self, filter), err)]
    pub async fn list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError> {
        self.scheduler.list_tasks(filter).await
    }

    /// Return per-queue statistics: pending count, running count, active workers.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), err)]
    pub async fn queue_statistics(
        &self,
        by_region: bool,
    ) -> Result<Vec<QueueStatistics>, TaskError> {
        self.scheduler.queue_statistics(by_region).await
    }

    /// Return per-worker status: `worker_id`, queue, tasks in flight.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), err)]
    pub async fn worker_status(&self) -> Result<Vec<WorkerStatus>, TaskError> {
        self.scheduler.worker_status().await
    }

    /// Query audit log entries for a given task.
    ///
    /// Returns an empty result when audit logging is disabled.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the database read fails.
    #[instrument(skip(self), fields(task_id = %task_id), err)]
    pub async fn audit_log(
        &self,
        task_id: TaskId,
        limit: i64,
        offset: i64,
    ) -> Result<iron_defer_domain::ListAuditLogResult, TaskError> {
        self.scheduler.audit_log(task_id, limit, offset).await
    }

    /// Start the worker pool and sweeper, blocking until the cancellation
    /// token fires and all in-flight tasks drain (or the drain timeout expires).
    ///
    /// The sweeper runs as an independent background task that recovers
    /// zombie tasks (running tasks with expired leases). Both the worker
    /// pool and sweeper share the same `CancellationToken` — when the
    /// token fires, the worker pool stops claiming new tasks.
    ///
    /// **Drain timeout (Architecture D6.1):** After the token fires, in-flight
    /// tasks are given `shutdown_timeout` (default 30s) to complete. If they
    /// finish in time, shutdown is clean. If the timeout expires, all leases
    /// held by this worker are released back to `Pending` via
    /// `release_leases_for_worker`, making them available for re-claiming.
    ///
    /// The caller is responsible for wiring the `CancellationToken` to OS
    /// signals. See [`shutdown::shutdown_signal`] for a ready-made future.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if a fatal repository error occurs
    /// during the poll loop. Sweeper errors are logged but not propagated.
    #[allow(clippy::too_many_lines)]
    #[instrument(skip(self, token), err)]
    pub async fn start(&self, token: CancellationToken) -> Result<(), TaskError> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::AlreadyStarted,
            });
        }

        let repo = Arc::new(PostgresTaskRepository::new(
            self.pool.clone(),
            self.audit_log,
        )) as Arc<dyn iron_defer_application::TaskRepository>;

        // Register observable gauges for pool stats + task counts.
        // Use the `Meter` embedded in `self.metrics` — the same meter that
        // built the synchronous instruments, attached to the caller-provided
        // Prometheus registry. Reaching for `global::meter(...)` would bind
        // to the default no-op provider unless the caller separately called
        // `set_meter_provider`, which the embedded library must not require
        // (Architecture §Enforcement Guidelines). The gauge callbacks read from a snapshot
        // refreshed by a background task on `token`, so no synchronous DB
        // query fires on the Prometheus scrape thread.
        let refresh_handle = self
            .metrics
            .as_ref()
            .map(|m| iron_defer_infrastructure::register_pool_gauges(m, &self.pool, &token));

        // D6.1: child tokens enable independent cancellation of subsystems.
        // Cancelling the parent `token` cancels both.
        let sweeper_token = token.child_token();
        let worker_token = token.child_token();

        let mut sweeper = SweeperService::new(
            repo.clone(),
            self.worker_config.sweeper_interval,
            self.worker_config.idempotency_key_retention,
            sweeper_token,
        )
        .with_suspend_timeout(self.worker_config.suspend_timeout)
        .with_saturation_classifier(std::sync::Arc::new(
            iron_defer_infrastructure::is_pool_timeout,
        ));
        if let Some(ref m) = self.metrics {
            sweeper = sweeper.with_metrics(m.clone());
        }

        let sweeper_handle = tokio::spawn(async move {
            if let Err(e) = sweeper.run().await {
                tracing::error!(error = %e, "sweeper exited with error");
            }
        });

        let worker_id = iron_defer_domain::WorkerId::new();
        let checkpoint_writer: std::sync::Arc<dyn iron_defer_domain::CheckpointWriter> =
            std::sync::Arc::new(iron_defer_infrastructure::PostgresCheckpointWriter::new(
                self.pool.clone(),
            ));
        let worker = WorkerService::builder()
            .repo(repo.clone())
            .registry(self.registry.clone())
            .config(self.worker_config.clone())
            .queue(self.queue.clone())
            .token(worker_token)
            .worker_id(worker_id)
            .is_saturation(std::sync::Arc::new(
                iron_defer_infrastructure::is_pool_timeout,
            ))
            .maybe_metrics(self.metrics.clone())
            .checkpoint_writer(checkpoint_writer)
            .build();

        // Phase 1: poll loop — runs until cancellation token fires.
        let mut join_set = match worker.run_poll_loop().await {
            Ok(js) => js,
            Err(e) => {
                // Ensure the sweeper is cancelled and joined before propagating,
                // otherwise it is leaked on the runtime.
                token.cancel();
                if let Err(join_err) = sweeper_handle.await {
                    tracing::error!(error = %join_err, "sweeper task panicked");
                }
                return Err(e);
            }
        };

        // Phase 2: drain with timeout (D6.1).
        let timeout = self.worker_config.shutdown_timeout;
        if tokio::time::timeout(
            timeout,
            iron_defer_application::drain_join_set(&mut join_set),
        )
        .await
        .is_err()
        {
            // Drain timeout expired. Order matters: abort first and wait for
            // futures to unwind BEFORE releasing leases, so no task is still
            // inside `repo.complete().await` when the release UPDATE runs.
            tracing::warn!(
                worker_id = %worker_id,
                timeout_secs = timeout.as_secs(),
                "drain timeout expired, aborting in-flight tasks"
            );
            join_set.abort_all();
            while let Some(res) = join_set.join_next().await {
                if let Err(e) = res
                    && e.is_panic()
                {
                    tracing::error!(error = %e, "in-flight task panicked during shutdown");
                }
                // Cancellation errors (non-panic JoinError) are expected here; ignore.
            }

            // Now release leases held by this worker — tasks return to Pending.
            match repo.release_leases_for_worker(worker_id).await {
                Ok(released) => {
                    tracing::warn!(
                        worker_id = %worker_id,
                        released_count = released.len(),
                        "released leases for timed-out in-flight tasks"
                    );
                    for (id, trace_id) in released {
                        iron_defer_application::emit_otel_state_transition(
                            trace_id.as_deref(),
                            id,
                            "running",
                            "pending",
                            "unknown", // queue/kind unknown at this site
                            "unknown",
                            Some(worker_id),
                            0, // attempts unknown
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(
                        worker_id = %worker_id,
                        error = %e,
                        "failed to release leases during shutdown timeout"
                    );
                }
            }
        }

        // Join sweeper — token is already cancelled so it will exit.
        if let Err(e) = sweeper_handle.await {
            tracing::error!(error = %e, "sweeper task panicked");
        }

        // Await the task-count refresh loop so its outstanding pool
        // connection (if any) is returned before `start()` returns.
        // Integration test suites that immediately rebuild an engine on
        // the same pool relied on this for clean hand-off; production
        // only notices on graceful shutdown.
        if let Some(h) = refresh_handle
            && let Err(e) = h.await
        {
            tracing::error!(error = %e, "task-count refresh loop panicked");
        }

        Ok(())
    }

    /// Start the axum HTTP server, blocking until the cancellation token
    /// fires and the server drains.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::Storage` if the TCP listener cannot bind.
    #[instrument(skip(self, token), fields(bind = %bind), err)]
    pub async fn serve(
        self: &Arc<Self>,
        bind: &str,
        token: CancellationToken,
    ) -> Result<(), TaskError> {
        let listener =
            tokio::net::TcpListener::bind(bind)
                .await
                .map_err(|e| TaskError::Storage {
                    source: Box::new(e),
                })?;

        let router = crate::http::router::build(Arc::clone(self));

        axum::serve(listener, router)
            .with_graceful_shutdown(token.cancelled_owned())
            .await
            .map_err(|e| TaskError::Storage {
                source: Box::new(e),
            })
    }

    /// Enqueue a task using runtime-typed `kind` and raw JSON payload.
    ///
    /// This is the bridge between the REST API (runtime strings) and the
    /// typed library API. Validates queue name, checks the registry for
    /// a matching handler, then delegates to `SchedulerService`.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if the queue name is invalid,
    /// no handler is registered for `kind`, or other validation fails.
    /// Returns `TaskError::Storage` if the database `INSERT` fails.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, payload), fields(queue = %queue, kind = %kind), err)]
    pub async fn enqueue_raw(
        &self,
        queue: &str,
        kind: &str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        priority: Option<i16>,
        max_attempts: Option<i32>,
        trace_id: Option<&str>,
        region: Option<&str>,
    ) -> Result<TaskRecord, TaskError> {
        self.validate_region_authorization(region)?;
        if kind.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "task kind must not be empty".to_string(),
                },
            });
        }

        if self.registry.get(kind).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("no handler registered for kind {kind:?}"),
                },
            });
        }

        if let Some(ma) = max_attempts
            && ma < 1
        {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("max_attempts must be >= 1, got {ma}"),
                },
            });
        }

        if let Some(ref dt) = scheduled_at {
            validate_scheduled_at(dt)?;
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;

        let record = self
            .scheduler
            .enqueue_raw(
                &queue_name,
                kind,
                payload,
                scheduled_at,
                priority,
                max_attempts,
                trace_id,
                region,
            )
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok(record)
    }

    /// Enqueue a task using runtime-typed kind with idempotency key support.
    ///
    /// Returns `(TaskRecord, created)` — `created=false` when a duplicate key exists.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, payload), fields(queue = %queue, kind = %kind, idempotency_key = %idempotency_key, region), err)]
    pub async fn enqueue_raw_idempotent(
        &self,
        queue: &str,
        kind: &str,
        payload: serde_json::Value,
        scheduled_at: Option<DateTime<Utc>>,
        priority: Option<i16>,
        max_attempts: Option<i32>,
        idempotency_key: &str,
        trace_id: Option<&str>,
        region: Option<&str>,
    ) -> Result<(TaskRecord, bool), TaskError> {
        self.validate_region_authorization(region)?;
        if idempotency_key.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "idempotency key must not be empty".to_owned(),
                },
            });
        }
        if idempotency_key.len() > 250 {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!(
                        "idempotency key length {} exceeds maximum of 250 characters",
                        idempotency_key.len()
                    ),
                },
            });
        }
        if kind.is_empty() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: "task kind must not be empty".to_string(),
                },
            });
        }

        if self.registry.get(kind).is_none() {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("no handler registered for kind {kind:?}"),
                },
            });
        }

        if let Some(ma) = max_attempts
            && ma < 1
        {
            return Err(TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("max_attempts must be >= 1, got {ma}"),
                },
            });
        }

        if let Some(ref dt) = scheduled_at {
            validate_scheduled_at(dt)?;
        }

        let queue_name = QueueName::try_from(queue).map_err(|e| TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!("invalid queue name: {e}"),
            },
        })?;

        let retention = self.worker_config.idempotency_key_retention;
        let (record, created) = self
            .scheduler
            .enqueue_raw_idempotent(
                &queue_name,
                kind,
                payload,
                scheduled_at,
                priority,
                max_attempts,
                idempotency_key,
                retention,
                trace_id,
                region,
            )
            .await?;

        emit_task_enqueued(&record, self.worker_config.log_payload);
        Ok((record, created))
    }
}

/// Postgres `timestamptz` valid range: 4713 BC (year −4712) to 294276 AD.
fn validate_region(region: &str) -> Result<(), TaskError> {
    if region.is_empty() {
        return Err(TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: "region label must not be empty".to_owned(),
            },
        });
    }
    if region.len() > 63 {
        return Err(TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!(
                    "region label length {} exceeds maximum of 63 characters",
                    region.len()
                ),
            },
        });
    }
    if !region
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!(
                    "region label '{region}' must contain only lowercase ASCII letters, digits, and hyphens"
                ),
            },
        });
    }
    Ok(())
}

fn validate_scheduled_at(dt: &DateTime<Utc>) -> Result<(), TaskError> {
    let year = dt.year();
    if !(-4712..=294_276).contains(&year) {
        return Err(TaskError::InvalidPayload {
            kind: PayloadErrorKind::Validation {
                message: format!(
                    "scheduled_at year {year} is outside Postgres timestamptz range (-4712..294276)"
                ),
            },
        });
    }
    Ok(())
}

/// Emit the `task_enqueued` lifecycle log with the payload field gated by
/// `log_payload`.
///
/// Kept at the module level instead of inlined to share one site between
/// `IronDefer::enqueue_inner` and `IronDefer::enqueue_raw` — identical
/// field layout simplifies downstream log-aggregator queries.
fn emit_task_enqueued(record: &TaskRecord, log_payload: bool) {
    // Use RFC 3339 / ISO 8601 so log aggregators can parse the value as a
    // timestamp field.
    let scheduled_at_iso = record.scheduled_at().to_rfc3339();
    // Emit `attempt = 0` explicitly at enqueue time so lifecycle events keep
    // a consistent schema.
    if log_payload {
        tracing::info!(
            event = "task_enqueued",
            task_id = %record.id(),
            queue = %record.queue(),
            kind = %record.kind(),
            priority = %record.priority(),
            attempt = 0_u32,
            max_attempts = %record.max_attempts(),
            scheduled_at = %scheduled_at_iso,
            payload = ?record.payload(),
            "task enqueued"
        );
    } else {
        tracing::info!(
            event = "task_enqueued",
            task_id = %record.id(),
            queue = %record.queue(),
            kind = %record.kind(),
            priority = %record.priority(),
            attempt = 0_u32,
            max_attempts = %record.max_attempts(),
            scheduled_at = %scheduled_at_iso,
            "task enqueued"
        );
    }
}

impl std::fmt::Debug for IronDefer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IronDefer")
            .field("registry", &self.registry)
            .finish_non_exhaustive()
    }
}

// ----------------------------------------------------------------------------
// IronDeferBuilder
// ----------------------------------------------------------------------------

/// Fluent builder for [`IronDefer`].
///
/// Construct via [`IronDefer::builder`]. The chain methods consume `self`
/// and return `Self` to enable the canonical Rust builder pattern:
///
/// ```ignore
/// IronDefer::builder()
///     .pool(pool)
///     .register::<MyTask>()
///     .build()
///     .await?;
/// ```
///
/// **Architectural invariants enforced here:**
/// - Caller provides the `PgPool` (Architecture §Architectural Boundaries — Public library API boundary).
/// - The builder never spawns a Tokio runtime (Architecture §Enforcement Guidelines).
/// - `TaskRegistry` is constructed in this crate only (Architecture §Process Patterns — TaskRegistry ownership)
///   — the [`Default`] impl below is the SOLE construction site in the
///   workspace outside of unit tests.
pub struct IronDeferBuilder {
    pool: Option<PgPool>,
    registry: TaskRegistry,
    skip_migrations: bool,
    worker_config: WorkerConfig,
    producer_cfg: iron_defer_application::ProducerConfig,
    database_config: DatabaseConfig,
    queue: Option<String>,
    metrics: Option<iron_defer_application::Metrics>,
    prometheus_registry: Option<prometheus::Registry>,
    readiness_timeout: std::time::Duration,
}

impl Default for IronDeferBuilder {
    fn default() -> Self {
        Self {
            pool: None,
            registry: TaskRegistry::new(),
            skip_migrations: false,
            worker_config: WorkerConfig::default(),
            producer_cfg: iron_defer_application::ProducerConfig::default(),
            database_config: DatabaseConfig::default(),
            queue: None,
            metrics: None,
            prometheus_registry: None,
            readiness_timeout: std::time::Duration::from_secs(5),
        }
    }
}

impl IronDeferBuilder {
    /// Provide the caller-owned `PgPool` the engine will use for all
    /// database access. Required — `build()` returns an error if no pool
    /// has been set.
    ///
    /// **Embedded callers**: if you construct your own pool, consider using
    /// `iron_defer_infrastructure::recommended_pool_options()` as a starting
    /// point. It pre-configures the hardened defaults (`test_before_acquire`,
    /// `idle_timeout`, `max_lifetime`, etc.) that the standalone engine uses.
    #[must_use]
    pub fn pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Register a task handler for type `T`. The handler is keyed by
    /// `T::KIND` in the registry; re-registering the same kind silently
    /// overwrites the previous entry.
    #[must_use]
    pub fn register<T: Task>(mut self) -> Self {
        let handler: Arc<dyn TaskHandler> = Arc::new(TaskHandlerAdapter::<T>(PhantomData));
        self.registry.register(handler);
        self
    }

    /// Override the default worker pool configuration.
    #[must_use]
    pub fn worker_config(mut self, config: WorkerConfig) -> Self {
        self.worker_config = config;
        self
    }

    /// Override the default producer configuration.
    #[must_use]
    pub fn producer_config(mut self, config: iron_defer_application::ProducerConfig) -> Self {
        self.producer_cfg = config;
        self
    }

    /// Override the default database configuration.
    #[must_use]
    pub fn database_config(mut self, config: DatabaseConfig) -> Self {
        self.database_config = config;
        self
    }

    /// Override the sweeper interval (how often zombie tasks are recovered).
    /// Defaults to 60 seconds.
    #[must_use]
    pub fn sweeper_interval(mut self, interval: std::time::Duration) -> Self {
        self.worker_config.sweeper_interval = interval;
        self
    }

    /// Override the shutdown drain timeout (how long to wait for in-flight
    /// tasks before releasing leases). Defaults to 30 seconds (Architecture D6.1).
    #[must_use]
    pub fn shutdown_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.worker_config.shutdown_timeout = timeout;
        self
    }

    /// Set the readiness probe timeout. Defaults to 5 seconds.
    #[must_use]
    pub fn readiness_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }

    /// Set the queue name to poll. Defaults to `"default"`.
    ///
    /// Validation is deferred to [`build()`](Self::build) so the builder
    /// chain stays infallible. If the name is invalid, `build()` returns
    /// `TaskError::InvalidPayload`.
    #[must_use]
    pub fn queue(mut self, name: &str) -> Self {
        self.queue = Some(name.to_owned());
        self
    }

    /// Provide `OTel` metric instrument handles for the worker and sweeper.
    ///
    /// Embedded callers create a [`Metrics`](iron_defer_application::Metrics)
    /// from their own `Meter` via [`create_metrics`](iron_defer_infrastructure::create_metrics).
    #[must_use]
    pub fn metrics(mut self, metrics: iron_defer_application::Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Provide a Prometheus registry for the `/metrics` scrape endpoint.
    ///
    /// When set, `GET /metrics` encodes all `OTel` metrics registered with
    /// this registry into Prometheus text format. When `None`, the endpoint
    /// returns 404.
    #[must_use]
    pub fn prometheus_registry(mut self, registry: prometheus::Registry) -> Self {
        self.prometheus_registry = Some(registry);
        self
    }

    /// Opt out of automatic migration on `build()`. The caller is then
    /// responsible for running [`IronDefer::migrator`] inside their own
    /// transaction.
    ///
    /// Defaults to `false`.
    #[must_use]
    pub fn skip_migrations(mut self, skip: bool) -> Self {
        self.skip_migrations = skip;
        self
    }

    /// Finalize the builder.
    ///
    /// 1. Verifies that a pool has been set.
    /// 2. Runs the embedded migration set against the pool unless
    ///    [`Self::skip_migrations`] was set to `true`.
    /// 3. Wires up the `PostgresTaskRepository` adapter, the
    ///    `SchedulerService`, and the registered handlers into a final
    ///    [`IronDefer`] engine.
    ///
    /// # Errors
    ///
    /// - `TaskError::Storage` if no pool was provided.
    /// - `TaskError::Storage` (boxed `MigrateError`) if the migration run
    ///   fails.
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded default queue name `"default"` fails
    /// `QueueName` validation (unreachable — `"default"` is a valid name).
    pub async fn build(self) -> Result<IronDefer, TaskError> {
        self.worker_config
            .validate()
            .map_err(|reason| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation { message: reason },
            })?;

        self.database_config
            .validate()
            .map_err(|reason| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation { message: reason },
            })?;

        let pool = self.pool.ok_or_else(|| TaskError::Storage {
            source: "PgPool not provided to IronDeferBuilder — call .pool(pool) before .build()"
                .into(),
        })?;

        if !self.skip_migrations {
            iron_defer_infrastructure::MIGRATOR
                .run(&pool)
                .await
                .map_err(|e| TaskError::Migration {
                    source: Box::new(e),
                })?;
        }

        // Post-migration: reconcile table persistence mode with config.
        Self::reconcile_table_persistence(&pool, self.database_config.unlogged_tables).await?;

        let pg_repo = Arc::new(PostgresTaskRepository::new(
            pool.clone(),
            self.database_config.audit_log,
        ));
        let repo = pg_repo.clone() as Arc<dyn iron_defer_application::TaskRepository>;
        let tx_repo = pg_repo as Arc<dyn iron_defer_application::TransactionalTaskRepository>;
        let scheduler = SchedulerService::new(repo).with_tx_repo(tx_repo);
        let registry = Arc::new(self.registry);
        let queue = match self.queue {
            Some(name) => {
                QueueName::try_from(name.as_str()).map_err(|e| TaskError::InvalidPayload {
                    kind: PayloadErrorKind::Validation {
                        message: format!("invalid queue name: {e}"),
                    },
                })?
            }
            None => QueueName::try_from("default").expect("\"default\" is a valid queue name"),
        };

        Ok(IronDefer {
            scheduler,
            registry,
            pool,
            worker_config: self.worker_config,
            producer_config: self.producer_cfg,
            queue,
            metrics: self.metrics,
            prometheus_registry: self.prometheus_registry,
            readiness_timeout: self.readiness_timeout,
            started: AtomicBool::new(false),
            audit_log: self.database_config.audit_log,
            unlogged_tables: self.database_config.unlogged_tables,
        })
    }

    async fn reconcile_table_persistence(
        pool: &PgPool,
        want_unlogged: bool,
    ) -> Result<(), TaskError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT relpersistence::text FROM pg_class WHERE relname = 'tasks'")
                .fetch_optional(pool)
                .await
                .map_err(|e| TaskError::Storage {
                    source: format!("failed to query table persistence: {e}").into(),
                })?;

        let Some((persistence,)) = row else {
            return Ok(());
        };

        let is_unlogged = persistence == "u";

        if want_unlogged && !is_unlogged {
            tracing::warn!(
                "converting tasks table to UNLOGGED — data will be LOST on Postgres crash recovery"
            );
            Self::set_table_persistence(pool, "UNLOGGED").await?;
        } else if !want_unlogged && is_unlogged {
            tracing::warn!("restoring tasks table to LOGGED (WAL-backed)");
            Self::set_table_persistence(pool, "LOGGED").await?;
        }

        if want_unlogged {
            tracing::warn!(
                "UNLOGGED table mode enabled — data will be LOST on Postgres crash recovery. \
                 Not suitable for durable workloads."
            );
            tracing::info!("tasks table persistence: UNLOGGED");
        } else {
            tracing::info!("tasks table persistence: LOGGED");
        }

        Ok(())
    }

    /// Change the `tasks` table persistence mode, handling FK constraints from
    /// `task_audit_log`. PostgreSQL forbids LOGGED tables from referencing
    /// UNLOGGED tables, so when converting to UNLOGGED we drop the FK
    /// (audit_log is disabled via mutual exclusion anyway). When restoring
    /// to LOGGED we re-add the FK so audit log integrity is preserved.
    async fn set_table_persistence(pool: &PgPool, mode: &str) -> Result<(), TaskError> {
        let has_audit_table: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = 'task_audit_log')",
        )
        .fetch_one(pool)
        .await
        .map_err(|e| TaskError::Storage {
            source: format!("failed to check task_audit_log existence: {e}").into(),
        })?;

        let fk_name: Option<String> = if has_audit_table {
            sqlx::query_scalar(
                "SELECT conname::text FROM pg_constraint \
                 WHERE conrelid = 'task_audit_log'::regclass \
                   AND confrelid = 'tasks'::regclass \
                   AND contype = 'f'",
            )
            .fetch_optional(pool)
            .await
            .map_err(|e| TaskError::Storage {
                source: format!("failed to find FK constraint: {e}").into(),
            })?
        } else {
            None
        };

        // Drop FK before conversion (required for UNLOGGED).
        if let Some(ref name) = fk_name {
            sqlx::query(&format!(
                "ALTER TABLE task_audit_log DROP CONSTRAINT IF EXISTS {name}"
            ))
            .execute(pool)
            .await
            .map_err(|e| TaskError::Storage {
                source: format!("failed to drop FK constraint: {e}").into(),
            })?;
        }

        sqlx::query(&format!("ALTER TABLE tasks SET {mode}"))
            .execute(pool)
            .await
            .map_err(|e| TaskError::Storage {
                source: format!("ALTER TABLE tasks SET {mode} failed: {e}").into(),
            })?;

        // Restore FK only when converting back to LOGGED (both tables are
        // now permanent, so the FK is valid).
        if mode == "LOGGED" {
            if let Some(ref name) = fk_name {
                // Delete orphaned audit rows before restoring the FK. This can
                // happen after a Postgres crash in UNLOGGED mode.
                sqlx::query(
                    "DELETE FROM task_audit_log WHERE NOT EXISTS \
                     (SELECT 1 FROM tasks WHERE tasks.id = task_audit_log.task_id)",
                )
                .execute(pool)
                .await
                .map_err(|e| TaskError::Storage {
                    source: format!("failed to clear orphaned audit logs: {e}").into(),
                })?;

                sqlx::query(&format!(
                    "ALTER TABLE task_audit_log ADD CONSTRAINT {name} \
                     FOREIGN KEY (task_id) REFERENCES tasks(id)"
                ))
                .execute(pool)
                .await
                .map_err(|e| TaskError::Storage {
                    source: format!("failed to restore FK constraint: {e}").into(),
                })?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iron_defer_domain::WorkerId;
    use serde::{Deserialize, Serialize};

    // Verify the builder default constructs an empty registry. The
    // builder cannot be exercised end-to-end without a real PgPool, so
    // the integration test binary in `tests/integration_test.rs` covers
    // the build() path.
    #[test]
    fn default_builder_has_empty_registry() {
        let builder = IronDeferBuilder::default();
        assert!(builder.registry.is_empty());
        assert!(builder.pool.is_none());
        assert!(!builder.skip_migrations);
    }

    #[test]
    fn skip_migrations_setter_round_trips() {
        let builder = IronDeferBuilder::default().skip_migrations(true);
        assert!(builder.skip_migrations);
    }

    /// Test fixture: a minimal `Task` impl used to exercise
    /// `TaskHandlerAdapter` directly.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct UnitTestTask {
        n: i32,
    }

    impl Task for UnitTestTask {
        const KIND: &'static str = "unit_test_task";

        async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
            Ok(())
        }
    }

    fn sample_ctx() -> TaskContext {
        TaskContext::new(
            TaskId::new(),
            WorkerId::new(),
            iron_defer_domain::AttemptCount::new(1).unwrap(),
        )
    }

    #[tokio::test]
    async fn task_handler_adapter_kind_matches_task_kind() {
        let adapter = TaskHandlerAdapter::<UnitTestTask>(PhantomData);
        assert_eq!(adapter.kind(), UnitTestTask::KIND);
    }

    #[tokio::test]
    async fn task_handler_adapter_executes_valid_payload() {
        let adapter: Arc<dyn TaskHandler> =
            Arc::new(TaskHandlerAdapter::<UnitTestTask>(PhantomData));
        let payload = serde_json::to_value(UnitTestTask { n: 42 }).expect("serialize");
        let ctx = sample_ctx();

        let result = adapter.execute(&payload, &ctx).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn task_handler_adapter_maps_serde_error_to_invalid_payload() {
        let adapter: Arc<dyn TaskHandler> =
            Arc::new(TaskHandlerAdapter::<UnitTestTask>(PhantomData));
        // Wrong shape: missing required field `n`.
        let bad_payload = serde_json::json!({"wrong": "shape"});
        let ctx = sample_ctx();

        let err = adapter
            .execute(&bad_payload, &ctx)
            .await
            .expect_err("malformed payload must error");
        match err {
            TaskError::InvalidPayload {
                kind: PayloadErrorKind::Deserialization { message },
            } => {
                assert!(
                    message.contains("missing field") || message.contains('n'),
                    "expected serde error mentioning the missing field, got: {message}"
                );
            }
            other => panic!("expected InvalidPayload::Deserialization, got {other:?}"),
        }
    }

    #[test]
    fn validate_scheduled_at_accepts_epoch() {
        let dt = chrono::DateTime::UNIX_EPOCH;
        assert!(validate_scheduled_at(&dt).is_ok());
    }

    #[test]
    fn validate_scheduled_at_accepts_far_future() {
        // chrono's max year (~262143) is within the Postgres range (294276)
        let dt = DateTime::<Utc>::MAX_UTC;
        assert!(validate_scheduled_at(&dt).is_ok());
    }

    #[test]
    fn validate_scheduled_at_rejects_beyond_pg_lower_bound() {
        let dt = DateTime::<Utc>::MIN_UTC;
        let err = validate_scheduled_at(&dt).unwrap_err();
        match err {
            TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation { message },
            } => {
                assert!(
                    message.contains("outside Postgres timestamptz range"),
                    "{message}"
                );
            }
            other => panic!("expected InvalidPayload::Validation, got {other:?}"),
        }
    }
}
