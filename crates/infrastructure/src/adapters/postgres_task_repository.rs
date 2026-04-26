//! Postgres-backed implementation of the `TaskRepository` port.
//!
//! All queries are compile-time verified via `sqlx::query!` / `sqlx::query_as!`
//! macros. CI builds use the committed `.sqlx/` offline cache; developer
//! workflows hit a live database via the `DATABASE_URL` env var.
//!
//! Architecture references:
//! - §D1.1 (Tasks Table Schema)
//! - §Process Patterns (Tracing instrumentation): `#[instrument]` rules
//! - §Architectural Boundaries (Database boundary): DB row types are
//!   `pub(crate)`; `TryFrom` mapping at the adapter boundary

use std::convert::TryFrom;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use iron_defer_application::{RecoveryOutcome, TaskRepository, TransactionalTaskRepository};
use iron_defer_domain::{
    AttemptCount, AuditLogEntry, CancelResult, ListTasksFilter, ListTasksResult, MaxAttempts,
    Priority, QueueName, QueueStatistics, TaskError, TaskId, TaskKind, TaskRecord, TaskStatus,
    ValidationError, WorkerId, WorkerStatus,
};
use sqlx::PgPool;
use tracing::{Span, field::Empty, instrument};
use uuid::Uuid;

use crate::error::PostgresAdapterError;

/// Maximum byte length retained in `TaskRecord::last_error` after mapping.
///
/// Adversarial error messages can balloon row size, log records, and metric
/// label cardinality. The 4 KiB cap is enforced at the adapter boundary so
/// the domain layer stays free of storage policy.
pub(crate) const LAST_ERROR_MAX_BYTES: usize = 4096;

/// Truncate a string to at most `LAST_ERROR_MAX_BYTES` bytes, preserving
/// UTF-8 character boundaries.
fn truncate_last_error(mut s: String) -> String {
    if s.len() <= LAST_ERROR_MAX_BYTES {
        return s;
    }
    // `str::floor_char_boundary` is stable since Rust 1.86 (`round_char_boundary`
    // feature stabilization); iron-defer's MSRV 1.94 is comfortably above.
    let cutoff = s.floor_char_boundary(LAST_ERROR_MAX_BYTES);
    s.truncate(cutoff);
    s
}

/// Variant of [`truncate_last_error`] that operates on `&str` and returns
/// a borrowed slice when no truncation is needed. Used by `save()` to cap
/// the value going INTO Postgres without an extra allocation in the common
/// case.
fn truncate_last_error_borrow(s: &str) -> &str {
    if s.len() <= LAST_ERROR_MAX_BYTES {
        return s;
    }
    let cutoff = s.floor_char_boundary(LAST_ERROR_MAX_BYTES);
    &s[..cutoff]
}

/// Internal database row type. Stays `pub(crate)` — never crosses the
/// infrastructure crate boundary (Architecture §Architectural Boundaries — Database boundary).
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)] // every field is read by TryFrom<TaskRow> for TaskRecord
pub(crate) struct TaskRow {
    pub id: Uuid,
    pub queue: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i16,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub scheduled_at: DateTime<Utc>,
    pub claimed_by: Option<Uuid>,
    pub claimed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub idempotency_key: Option<String>,
    pub idempotency_expires_at: Option<DateTime<Utc>>,
    pub trace_id: Option<String>,
    pub checkpoint: Option<serde_json::Value>,
    pub suspended_at: Option<DateTime<Utc>>,
    pub signal_payload: Option<serde_json::Value>,
    pub region: Option<String>,
}

/// Extended row type that includes a window-function `total_count` column.
/// Used exclusively by `list_tasks` which needs `COUNT(*) OVER()`.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub(crate) struct TaskRowWithCount {
    pub id: Uuid,
    pub queue: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i16,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub scheduled_at: DateTime<Utc>,
    pub claimed_by: Option<Uuid>,
    pub claimed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub idempotency_key: Option<String>,
    pub idempotency_expires_at: Option<DateTime<Utc>>,
    pub trace_id: Option<String>,
    pub checkpoint: Option<serde_json::Value>,
    pub suspended_at: Option<DateTime<Utc>>,
    pub signal_payload: Option<serde_json::Value>,
    pub region: Option<String>,
    pub total_count: Option<i64>,
}

impl From<TaskRowWithCount> for TaskRow {
    fn from(r: TaskRowWithCount) -> Self {
        Self {
            id: r.id,
            queue: r.queue,
            kind: r.kind,
            payload: r.payload,
            status: r.status,
            priority: r.priority,
            attempts: r.attempts,
            max_attempts: r.max_attempts,
            last_error: r.last_error,
            scheduled_at: r.scheduled_at,
            claimed_by: r.claimed_by,
            claimed_until: r.claimed_until,
            created_at: r.created_at,
            updated_at: r.updated_at,
            idempotency_key: r.idempotency_key,
            idempotency_expires_at: r.idempotency_expires_at,
            trace_id: r.trace_id,
            checkpoint: r.checkpoint,
            suspended_at: r.suspended_at,
            signal_payload: r.signal_payload,
            region: r.region,
        }
    }
}

impl TryFrom<TaskRow> for TaskRecord {
    type Error = PostgresAdapterError;

    fn try_from(row: TaskRow) -> Result<Self, Self::Error> {
        let kind = TaskKind::try_from(row.kind).map_err(|e: ValidationError| {
            PostgresAdapterError::Mapping {
                reason: format!("invalid task kind: {e}"),
            }
        })?;

        let attempts =
            AttemptCount::new(row.attempts).map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("task.attempts: {e}"),
            })?;
        let max_attempts =
            MaxAttempts::new(row.max_attempts).map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("task.max_attempts: {e}"),
            })?;

        if attempts.get() > max_attempts.get() {
            return Err(PostgresAdapterError::Mapping {
                reason: format!(
                    "attempts ({}) exceeds max_attempts ({}) for task {}",
                    attempts.get(),
                    max_attempts.get(),
                    row.id
                ),
            });
        }

        let priority = Priority::new(row.priority);

        let queue = QueueName::try_from(row.queue).map_err(|e: ValidationError| {
            PostgresAdapterError::Mapping {
                reason: format!("invalid queue name: {e}"),
            }
        })?;

        let status = parse_status(&row.status)?;

        let last_error = row.last_error.map(truncate_last_error);
        let claimed_by = row.claimed_by.map(WorkerId::from_uuid);

        Ok(TaskRecord::builder()
            .id(TaskId::from_uuid(row.id))
            .queue(queue)
            .kind(kind)
            .payload(Arc::new(row.payload))
            .status(status)
            .priority(priority)
            .attempts(attempts)
            .max_attempts(max_attempts)
            .maybe_last_error(last_error)
            .scheduled_at(row.scheduled_at)
            .maybe_claimed_by(claimed_by)
            .maybe_claimed_until(row.claimed_until)
            .created_at(row.created_at)
            .updated_at(row.updated_at)
            .maybe_idempotency_key(row.idempotency_key)
            .maybe_idempotency_expires_at(row.idempotency_expires_at)
            .maybe_trace_id(row.trace_id)
            .maybe_checkpoint(row.checkpoint.map(Arc::new))
            .maybe_suspended_at(row.suspended_at)
            .maybe_signal_payload(row.signal_payload.map(Arc::new))
            .maybe_region(row.region)
            .build())
    }
}

fn parse_status(s: &str) -> Result<TaskStatus, PostgresAdapterError> {
    match s.to_ascii_lowercase().as_str() {
        "pending" => Ok(TaskStatus::Pending),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        "suspended" => Ok(TaskStatus::Suspended),
        other => Err(PostgresAdapterError::Mapping {
            reason: format!("unknown task status: {other:?}"),
        }),
    }
}

fn status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Suspended => "suspended",
        _ => "unknown",
    }
}

/// Checkpoint writer backed by Postgres.
///
/// Implements the domain `CheckpointWriter` trait using a separate pool
/// connection (not the worker's SKIP LOCKED claim connection).
#[derive(Clone)]
pub struct PostgresCheckpointWriter {
    pool: PgPool,
}

impl PostgresCheckpointWriter {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl iron_defer_domain::CheckpointWriter for PostgresCheckpointWriter {
    fn write_checkpoint(
        &self,
        task_id: TaskId,
        worker_id: WorkerId,
        data: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TaskError>> + Send + '_>>
    {
        Box::pin(async move {
            let result = sqlx::query!(
                r#"UPDATE tasks SET checkpoint = $1, updated_at = now() WHERE id = $2 AND claimed_by = $3 AND status = 'running'"#,
                data,
                task_id.as_uuid(),
                worker_id.as_uuid(),
            )
            .execute(&self.pool)
            .await
            .map_err(|e| {
                let adapter_err: PostgresAdapterError = e.into();
                TaskError::from(adapter_err)
            })?;

            if result.rows_affected() == 0 {
                return Err(TaskError::NotInExpectedState {
                    task_id,
                    expected: "Running and claimed by current worker",
                });
            }

            Ok(())
        })
    }
}

/// Maximum offset allowed for pagination to prevent deep-paging performance
/// degradation (NFR-P1).
const MAX_PAGINATION_OFFSET: u32 = 10_000;

/// Standard task columns for SQL queries to reduce duplication.
const TASK_COLUMNS: &str = "id, queue, kind, payload, status, priority, attempts, max_attempts, last_error, scheduled_at, claimed_by, claimed_until, created_at, updated_at, idempotency_key, idempotency_expires_at, trace_id, checkpoint, suspended_at, signal_payload, region";

/// Postgres-backed `TaskRepository` adapter.
///
/// Holds a cloned `PgPool` (cheap `Arc` clone). The pool itself is owned by
/// the caller per the architecture's "caller-provided `PgPool`" rule
/// (Architecture §Architectural Boundaries — Public library API boundary).
#[derive(Debug, Clone)]
pub struct PostgresTaskRepository {
    pool: PgPool,
    audit_log: bool,
}

impl PostgresTaskRepository {
    #[must_use]
    pub fn new(pool: PgPool, audit_log: bool) -> Self {
        Self { pool, audit_log }
    }

    #[instrument(skip(self, tx, metadata), err)]
    async fn insert_audit_row(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        task_id: Uuid,
        from_status: Option<&str>,
        to_status: &str,
        worker_id: Option<Uuid>,
        trace_id: Option<&str>,
        metadata: Option<serde_json::Value>,
        region: Option<&str>,
    ) -> Result<(), TaskError> {
        if !self.audit_log {
            return Ok(());
        }

        // Cap trace_id to match VARCHAR(255) constraint.
        let capped_trace_id = trace_id.map(|t| if t.len() > 255 { &t[..255] } else { t });

        // Cap metadata size to prevent database bloat (e.g., 10 KB)
        let capped_metadata = metadata.and_then(|m| {
            let s = serde_json::to_string(&m).unwrap_or_default();
            if s.len() > 10240 {
                // If too large, record a truncation warning instead of the full payload
                Some(serde_json::json!({
                    "warning": "metadata truncated: exceeded 10KB limit",
                    "original_size": s.len()
                }))
            } else {
                Some(m)
            }
        });

        sqlx::query(
            "INSERT INTO task_audit_log (task_id, from_status, to_status, worker_id, trace_id, metadata, region) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(task_id)
        .bind(from_status)
        .bind(to_status)
        .bind(worker_id)
        .bind(capped_trace_id)
        .bind(capped_metadata)
        .bind(region)
        .execute(&mut **tx)
        .await
        .map_err(PostgresAdapterError::from)?;
        Ok(())
    }
}

#[async_trait]
impl TaskRepository for PostgresTaskRepository {
    #[instrument(
        skip(self, task),
        fields(task_id = %task.id(), queue = %task.queue(), kind = %task.kind()),
        err
    )]
    async fn save(&self, task: &TaskRecord) -> Result<TaskRecord, TaskError> {
        let last_error_capped = task.last_error().map(truncate_last_error_borrow);

        let id = task.id();
        let payload = task.payload();
        let status = status_to_str(task.status());
        let priority = task.priority().get();
        let attempts = task.attempts().get();
        let max_attempts = task.max_attempts().get();
        let scheduled_at = task.scheduled_at();
        let claimed_by = task.claimed_by().as_ref().map(|w| *w.as_uuid());
        let claimed_until = task.claimed_until();
        let idempotency_key = task.idempotency_key();
        let idempotency_expires_at = task.idempotency_expires_at();
        let trace_id = task.trace_id();

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            INSERT INTO tasks (
                id, queue, kind, payload, status, priority,
                attempts, max_attempts, last_error,
                scheduled_at, claimed_by, claimed_until,
                idempotency_key, idempotency_expires_at, trace_id,
                checkpoint, suspended_at, signal_payload,
                region
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
            RETURNING *
            "#,
            id.as_uuid(),
            task.queue().as_str(),
            task.kind().as_str(),
            payload,
            status,
            priority,
            attempts,
            max_attempts,
            last_error_capped,
            scheduled_at,
            claimed_by,
            claimed_until,
            idempotency_key,
            idempotency_expires_at,
            trace_id,
            task.checkpoint(),
            task.suspended_at(),
            task.signal_payload(),
            task.region(),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        self.insert_audit_row(
            &mut tx,
            row.id,
            None,
            "pending",
            None,
            row.trace_id.as_deref(),
            None,
            row.region.as_deref(),
        )
        .await?;

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        let record = TaskRecord::try_from(row)?;
        Ok(record)
    }

    #[instrument(
        skip(self, task),
        fields(task_id = %task.id(), queue = %task.queue(), idempotency_key = ?task.idempotency_key()),
        err
    )]
    async fn save_idempotent(&self, task: &TaskRecord) -> Result<(TaskRecord, bool), TaskError> {
        let last_error_capped = task.last_error().map(truncate_last_error_borrow);

        let id = task.id();
        let payload = task.payload();
        let status = status_to_str(task.status());
        let priority = task.priority().get();
        let attempts = task.attempts().get();
        let max_attempts = task.max_attempts().get();
        let scheduled_at = task.scheduled_at();
        let claimed_by = task.claimed_by().as_ref().map(|w| *w.as_uuid());
        let claimed_until = task.claimed_until();
        let idempotency_key = task.idempotency_key();
        let idempotency_expires_at = task.idempotency_expires_at();
        let trace_id = task.trace_id();

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let inserted_row = sqlx::query_as!(
            TaskRow,
            r#"
            INSERT INTO tasks (
                id, queue, kind, payload, status, priority,
                attempts, max_attempts, last_error,
                scheduled_at, claimed_by, claimed_until,
                idempotency_key, idempotency_expires_at, trace_id,
                checkpoint, suspended_at, signal_payload,
                region
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
            ON CONFLICT (queue, idempotency_key)
                WHERE idempotency_key IS NOT NULL
                  AND status NOT IN ('completed', 'failed', 'cancelled')
            DO NOTHING
            RETURNING *
            "#,
            id.as_uuid(),
            task.queue().as_str(),
            task.kind().as_str(),
            payload,
            status,
            priority,
            attempts,
            max_attempts,
            last_error_capped,
            scheduled_at,
            claimed_by,
            claimed_until,
            idempotency_key,
            idempotency_expires_at,
            trace_id,
            task.checkpoint(),
            task.suspended_at(),
            task.signal_payload(),
            task.region(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        if let Some(row) = inserted_row {
            self.insert_audit_row(
                &mut tx,
                row.id,
                None,
                "pending",
                None,
                row.trace_id.as_deref(),
                None,
                row.region.as_deref(),
            )
            .await?;
            tx.commit().await.map_err(PostgresAdapterError::from)?;
            let record = TaskRecord::try_from(row)?;
            return Ok((record, true));
        }

        // Conflict — fetch the existing task
        let existing_row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT * FROM tasks
            WHERE queue = $1 AND idempotency_key = $2
              AND status NOT IN ('completed', 'failed', 'cancelled')
            "#,
            task.queue().as_str(),
            idempotency_key,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        match existing_row {
            Some(row) => {
                let record = TaskRecord::try_from(row)?;
                Ok((record, false))
            }
            None => Err(TaskError::NotInExpectedState {
                task_id: TaskId::from_uuid(id.as_uuid().to_owned()),
                expected: "non-terminal",
            }),
        }
    }

    #[instrument(skip(self), err)]
    async fn cleanup_expired_idempotency_keys(&self) -> Result<u64, TaskError> {
        let now = Utc::now();
        let result = sqlx::query!(
            r#"
            UPDATE tasks
            SET idempotency_key = NULL,
                idempotency_expires_at = NULL,
                updated_at = now()
            WHERE idempotency_expires_at < $1
              AND status IN ('completed', 'failed', 'cancelled')
              AND idempotency_key IS NOT NULL
            "#,
            now,
        )
        .execute(&self.pool)
        .await
        .map_err(PostgresAdapterError::from)?;

        Ok(result.rows_affected())
    }

    #[instrument(skip(self), fields(task_id = %id), err)]
    async fn find_by_id(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError> {
        let row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT * FROM tasks
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(PostgresAdapterError::from)?;

        row.map(TaskRecord::try_from)
            .transpose()
            .map_err(TaskError::from)
    }

    #[instrument(skip(self), fields(queue = %queue), err)]
    async fn list_by_queue(&self, queue: &QueueName) -> Result<Vec<TaskRecord>, TaskError> {
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM tasks \
             WHERE queue = $1 \
             ORDER BY created_at ASC, id ASC"
        );
        let rows = sqlx::query_as::<_, TaskRow>(&sql)
            .bind(queue.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(PostgresAdapterError::from)?;

        rows.into_iter()
            .map(TaskRecord::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(TaskError::from)
    }

    #[instrument(skip(self), fields(queue = %queue, worker_id = %worker_id), err)]
    async fn claim_next(
        &self,
        queue: &QueueName,
        worker_id: WorkerId,
        lease_duration: Duration,
        region: Option<&str>,
    ) -> Result<Option<TaskRecord>, TaskError> {
        let lease_secs = lease_duration.as_secs_f64();

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = if let Some(region) = region {
            sqlx::query_as!(
                TaskRow,
                r#"
                UPDATE tasks
                SET status = 'running',
                    claimed_by = $1,
                    claimed_until = now() + make_interval(secs => $2),
                    attempts = attempts + 1,
                    updated_at = now()
                WHERE id = (
                    SELECT id FROM tasks
                    WHERE queue = $3
                      AND status = 'pending'
                      AND scheduled_at <= now()
                      AND (region IS NULL OR region = $4)
                    ORDER BY priority DESC, scheduled_at ASC
                    FOR UPDATE SKIP LOCKED
                    LIMIT 1
                )
                RETURNING *
                "#,
                worker_id.as_uuid(),
                lease_secs,
                queue.as_str(),
                region,
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(PostgresAdapterError::from)?
        } else {
            sqlx::query_as!(
                TaskRow,
                r#"
                UPDATE tasks
                SET status = 'running',
                    claimed_by = $1,
                    claimed_until = now() + make_interval(secs => $2),
                    attempts = attempts + 1,
                    updated_at = now()
                WHERE id = (
                    SELECT id FROM tasks
                    WHERE queue = $3
                      AND status = 'pending'
                      AND scheduled_at <= now()
                      AND region IS NULL
                    ORDER BY priority DESC, scheduled_at ASC
                    FOR UPDATE SKIP LOCKED
                    LIMIT 1
                )
                RETURNING *
                "#,
                worker_id.as_uuid(),
                lease_secs,
                queue.as_str(),
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(PostgresAdapterError::from)?
        };

        if let Some(ref row) = row {
            self.insert_audit_row(
                &mut tx,
                row.id,
                Some("pending"),
                "running",
                Some(*worker_id.as_uuid()),
                row.trace_id.as_deref(),
                None,
                row.region.as_deref(),
            )
            .await?;
        }

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        row.map(TaskRecord::try_from)
            .transpose()
            .map_err(TaskError::from)
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    async fn complete(&self, task_id: TaskId) -> Result<TaskRecord, TaskError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            UPDATE tasks
            SET status = 'completed',
                checkpoint = NULL,
                updated_at = now()
            WHERE id = $1
              AND status = 'running'
            RETURNING *
            "#,
            task_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match row {
            Some(r) => {
                self.insert_audit_row(
                    &mut tx,
                    r.id,
                    Some("running"),
                    "completed",
                    r.claimed_by,
                    r.trace_id.as_deref(),
                    None,
                    r.region.as_deref(),
                )
                .await?;
                tx.commit().await.map_err(PostgresAdapterError::from)?;
                Ok(TaskRecord::try_from(r)?)
            }
            None => Err(TaskError::NotInExpectedState {
                task_id,
                expected: "Running",
            }),
        }
    }

    #[instrument(skip(self, error_message), fields(task_id = %task_id), err)]
    async fn fail(
        &self,
        task_id: TaskId,
        error_message: &str,
        base_delay_secs: f64,
        max_delay_secs: f64,
    ) -> Result<TaskRecord, TaskError> {
        let truncated_error = truncate_last_error(error_message.to_owned());
        debug_assert!(
            base_delay_secs.is_finite() && base_delay_secs > 0.0,
            "base_delay_secs must be finite and positive"
        );
        debug_assert!(
            max_delay_secs.is_finite() && max_delay_secs > 0.0,
            "max_delay_secs must be finite and positive"
        );
        debug_assert!(
            max_delay_secs >= base_delay_secs,
            "max_delay_secs must be >= base_delay_secs"
        );

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            UPDATE tasks
            SET status = CASE
                    WHEN attempts < max_attempts THEN 'pending'
                    ELSE 'failed'
                END,
                claimed_by = CASE
                    WHEN attempts < max_attempts THEN NULL
                    ELSE claimed_by
                END,
                claimed_until = CASE
                    WHEN attempts < max_attempts THEN NULL
                    ELSE claimed_until
                END,
                last_error = $2,
                scheduled_at = CASE
                    WHEN attempts < max_attempts
                    THEN now() + make_interval(secs =>
                        LEAST($3 * power(2, attempts - 1), $4)
                        * (0.75 + random() * 0.5))
                    ELSE scheduled_at
                END,
                updated_at = now()
            WHERE id = $1
              AND status = 'running'
            RETURNING *
            "#,
            task_id.as_uuid(),
            &truncated_error,
            base_delay_secs,
            max_delay_secs,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match row {
            Some(r) => {
                let to_status = &r.status;
                let metadata = serde_json::json!({"error": truncated_error});
                self.insert_audit_row(
                    &mut tx,
                    r.id,
                    Some("running"),
                    to_status,
                    r.claimed_by,
                    r.trace_id.as_deref(),
                    Some(metadata),
                    r.region.as_deref(),
                )
                .await?;
                tx.commit().await.map_err(PostgresAdapterError::from)?;
                Ok(TaskRecord::try_from(r)?)
            }
            None => Err(TaskError::NotInExpectedState {
                task_id,
                expected: "Running",
            }),
        }
    }

    #[instrument(skip(self), err)]
    async fn recover_zombie_tasks(
        &self,
    ) -> Result<Vec<(TaskId, QueueName, TaskKind, Option<String>, RecoveryOutcome)>, TaskError>
    {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        // Intentionally excludes 'suspended' tasks (G7 HITL — suspend watchdog handles timeout separately)
        let retryable_rows = sqlx::query!(
            r#"
            UPDATE tasks
            SET status = 'pending',
                claimed_by = NULL,
                claimed_until = NULL,
                scheduled_at = now(),
                updated_at = now()
            WHERE status = 'running'
              AND claimed_until < now()
              AND attempts < max_attempts
            RETURNING id, queue, kind, trace_id, region
            "#,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        let exhausted_rows = sqlx::query!(
            r#"
            UPDATE tasks
            SET status = 'failed',
                last_error = 'lease expired: max attempts exhausted',
                updated_at = now()
            WHERE status = 'running'
              AND claimed_until < now()
              AND attempts >= max_attempts
            RETURNING id, queue, kind, trace_id, region
            "#,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        let mut results = Vec::with_capacity(retryable_rows.len() + exhausted_rows.len());
        for row in &retryable_rows {
            self.insert_audit_row(
                &mut tx,
                row.id,
                Some("running"),
                "pending",
                None,
                row.trace_id.as_deref(),
                None,
                row.region.as_deref(),
            )
            .await?;
        }
        for row in &exhausted_rows {
            let metadata = serde_json::json!({"error": "lease expired: max attempts exhausted"});
            self.insert_audit_row(
                &mut tx,
                row.id,
                Some("running"),
                "failed",
                None,
                row.trace_id.as_deref(),
                Some(metadata),
                row.region.as_deref(),
            )
            .await?;
        }

        for row in retryable_rows {
            let id = TaskId::from_uuid(row.id);
            let queue =
                QueueName::try_from(row.queue).map_err(|e| PostgresAdapterError::Mapping {
                    reason: format!("invalid queue name in recovered task {id}: {e}"),
                })?;
            let kind = TaskKind::try_from(row.kind).map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("invalid kind in recovered task {id}: {e}"),
            })?;
            results.push((id, queue, kind, row.trace_id, RecoveryOutcome::Recovered));
        }

        for row in exhausted_rows {
            let id = TaskId::from_uuid(row.id);
            let queue =
                QueueName::try_from(row.queue).map_err(|e| PostgresAdapterError::Mapping {
                    reason: format!("invalid queue name in exhausted task {id}: {e}"),
                })?;
            let kind = TaskKind::try_from(row.kind).map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("invalid kind in exhausted task {id}: {e}"),
            })?;
            results.push((id, queue, kind, row.trace_id, RecoveryOutcome::Failed));
        }

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        Ok(results)
    }

    #[instrument(skip(self, filter), fields(
        queue = filter.queue.as_ref().map_or("*", |q| q.as_str()),
        status = filter.status.map_or("*", status_to_str),
        limit = filter.limit,
        offset = filter.offset,
    ), err)]
    async fn list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError> {
        let mut where_clauses = Vec::new();

        if filter.queue.is_some() {
            where_clauses.push("queue = $1");
        }
        if filter.status.is_some() {
            let idx = if filter.queue.is_some() { 2 } else { 1 };
            where_clauses.push(if idx == 1 {
                "status = $1"
            } else {
                "status = $2"
            });
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        // Determine parameter positions for LIMIT and OFFSET
        let mut next_idx = 1;
        if filter.queue.is_some() {
            next_idx += 1;
        }
        if filter.status.is_some() {
            next_idx += 1;
        }
        let limit_idx = next_idx;
        let offset_idx = next_idx + 1;

        let offset = filter.offset.min(MAX_PAGINATION_OFFSET);

        let sql = format!(
            "SELECT *, COUNT(*) OVER() AS total_count \
             FROM tasks {where_sql} \
             ORDER BY created_at ASC, id ASC \
             LIMIT ${limit_idx} OFFSET ${offset_idx}"
        );

        let mut query = sqlx::query_as::<_, TaskRowWithCount>(&sql);
        if let Some(ref queue) = filter.queue {
            query = query.bind(queue.as_str());
        }
        if let Some(status) = filter.status {
            query = query.bind(status_to_str(status));
        }
        query = query.bind(i64::from(filter.limit)).bind(i64::from(offset));

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(PostgresAdapterError::from)?;

        #[allow(clippy::cast_sign_loss)]
        let total = if let Some(first) = rows.first() {
            first.total_count.unwrap_or(0) as u64
        } else if filter.offset == 0 {
            0
        } else {
            // Fallback: if offset > 0 and no rows returned, window function
            // total_count is not available. Run a dedicated count query to
            // ensure pagination metadata remains correct.
            let count_sql = format!("SELECT COUNT(*) FROM tasks {where_sql}");
            let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
            if let Some(ref queue) = filter.queue {
                count_query = count_query.bind(queue.as_str());
            }
            if let Some(status) = filter.status {
                count_query = count_query.bind(status_to_str(status));
            }
            count_query
                .fetch_one(&self.pool)
                .await
                .map_err(PostgresAdapterError::from)? as u64
        };

        let tasks = rows
            .into_iter()
            .map(|r| TaskRecord::try_from(TaskRow::from(r)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(TaskError::from)?;

        Ok(ListTasksResult { tasks, total })
    }

    #[instrument(skip(self), err)]
    async fn queue_statistics(&self, by_region: bool) -> Result<Vec<QueueStatistics>, TaskError> {
        tracing::debug!("fetching queue statistics (by_region: {by_region})");
        #[derive(sqlx::FromRow)]
        struct QueueStatsRow {
            queue: String,
            region: Option<String>,
            pending: i64,
            running: i64,
            suspended: i64,
            active_workers: i64,
        }

        let sql = if by_region {
            "SELECT \
                 queue, \
                 region, \
                 COUNT(*) FILTER (WHERE status = 'pending') as pending, \
                 COUNT(*) FILTER (WHERE status = 'running') as running, \
                 COUNT(*) FILTER (WHERE status = 'suspended') as suspended, \
                 COUNT(DISTINCT claimed_by) FILTER (WHERE status = 'running') as active_workers \
             FROM tasks \
             GROUP BY queue, region \
             HAVING COUNT(*) FILTER (WHERE status IN ('pending', 'running', 'suspended')) > 0 \
             ORDER BY queue, region"
                .to_string()
        } else {
            "SELECT \
                 queue, \
                 NULL::VARCHAR as region, \
                 COUNT(*) FILTER (WHERE status = 'pending') as pending, \
                 COUNT(*) FILTER (WHERE status = 'running') as running, \
                 COUNT(*) FILTER (WHERE status = 'suspended') as suspended, \
                 COUNT(DISTINCT claimed_by) FILTER (WHERE status = 'running') as active_workers \
             FROM tasks \
             GROUP BY queue \
             HAVING COUNT(*) FILTER (WHERE status IN ('pending', 'running', 'suspended')) > 0 \
             ORDER BY queue"
                .to_string()
        };

        let rows: Vec<QueueStatsRow> = sqlx::query_as(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(PostgresAdapterError::from)?;

        let stats = rows
            .into_iter()
            .map(|row| {
                let queue = QueueName::try_from(row.queue).map_err(|e: ValidationError| {
                    PostgresAdapterError::Mapping {
                        reason: format!("invalid queue name in stats: {e}"),
                    }
                })?;
                #[allow(clippy::cast_sign_loss)]
                Ok(QueueStatistics {
                    queue,
                    region: row.region,
                    pending: row.pending as u64,
                    running: row.running as u64,
                    suspended: row.suspended as u64,
                    active_workers: row.active_workers as u64,
                })
            })
            .collect::<Result<Vec<_>, PostgresAdapterError>>()
            .map_err(TaskError::from)?;

        Ok(stats)
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    async fn cancel(&self, task_id: TaskId) -> Result<CancelResult, TaskError> {
        #[derive(sqlx::FromRow)]
        struct CancelRow {
            id: Option<Uuid>,
            queue: Option<String>,
            kind: Option<String>,
            payload: Option<serde_json::Value>,
            status: Option<String>,
            priority: Option<i16>,
            attempts: Option<i32>,
            max_attempts: Option<i32>,
            last_error: Option<String>,
            scheduled_at: Option<DateTime<Utc>>,
            claimed_by: Option<Uuid>,
            claimed_until: Option<DateTime<Utc>>,
            created_at: Option<DateTime<Utc>>,
            updated_at: Option<DateTime<Utc>>,
            idempotency_key: Option<String>,
            idempotency_expires_at: Option<DateTime<Utc>>,
            trace_id: Option<String>,
            checkpoint: Option<serde_json::Value>,
            suspended_at: Option<DateTime<Utc>>,
            signal_payload: Option<serde_json::Value>,
            region: Option<String>,
            original_status: Option<String>,
            was_cancelled: Option<bool>,
            task_exists: Option<bool>,
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row: CancelRow = sqlx::query_as(
            r"
            WITH cancel_attempt AS (
                UPDATE tasks
                SET status = 'cancelled', updated_at = now()
                WHERE id = $1 AND (status = 'pending' OR status = 'suspended')
                RETURNING *
            )
            SELECT
                ca.id, ca.queue, ca.kind, ca.payload, ca.status, ca.priority,
                ca.attempts, ca.max_attempts, ca.last_error,
                ca.scheduled_at, ca.claimed_by, ca.claimed_until,
                ca.created_at, ca.updated_at,
                ca.idempotency_key, ca.idempotency_expires_at,
                ca.trace_id, ca.checkpoint,
                ca.suspended_at, ca.signal_payload,
                ca.region,
                t.status AS original_status,
                (ca.id IS NOT NULL) AS was_cancelled,
                (t.id IS NOT NULL) AS task_exists
            FROM (SELECT $1::uuid AS lookup_id) params
            LEFT JOIN cancel_attempt ca ON ca.id = params.lookup_id
            LEFT JOIN tasks t ON t.id = params.lookup_id AND ca.id IS NULL
            ",
        )
        .bind(task_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        if row.was_cancelled == Some(true) {
            let cancelled_id = row.id.expect("invariant: cancelled row has id");
            let from_status = if row.suspended_at.is_some() {
                "suspended"
            } else {
                "pending"
            };
            self.insert_audit_row(
                &mut tx,
                cancelled_id,
                Some(from_status),
                "cancelled",
                None,
                row.trace_id.as_deref(),
                None,
                row.region.as_deref(),
            )
            .await?;
            tx.commit().await.map_err(PostgresAdapterError::from)?;

            let task_row = TaskRow {
                id: cancelled_id,
                queue: row.queue.expect("invariant: cancelled row has queue"),
                kind: row.kind.expect("invariant: cancelled row has kind"),
                payload: row.payload.expect("invariant: cancelled row has payload"),
                status: row.status.expect("invariant: cancelled row has status"),
                priority: row.priority.expect("invariant: cancelled row has priority"),
                attempts: row.attempts.expect("invariant: cancelled row has attempts"),
                max_attempts: row
                    .max_attempts
                    .expect("invariant: cancelled row has max_attempts"),
                last_error: row.last_error,
                scheduled_at: row
                    .scheduled_at
                    .expect("invariant: cancelled row has scheduled_at"),
                claimed_by: row.claimed_by,
                claimed_until: row.claimed_until,
                created_at: row
                    .created_at
                    .expect("invariant: cancelled row has created_at"),
                updated_at: row
                    .updated_at
                    .expect("invariant: cancelled row has updated_at"),
                idempotency_key: row.idempotency_key,
                idempotency_expires_at: row.idempotency_expires_at,
                trace_id: row.trace_id,
                checkpoint: row.checkpoint,
                suspended_at: row.suspended_at,
                signal_payload: row.signal_payload,
                region: row.region,
            };
            let record = TaskRecord::try_from(task_row)?;
            return Ok(CancelResult::Cancelled(record));
        }

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        if row.task_exists == Some(true) {
            let status_str = row
                .original_status
                .expect("invariant: existing task has status");
            let status = parse_status(&status_str)?;
            return Ok(CancelResult::NotCancellable {
                current_status: status,
            });
        }

        Ok(CancelResult::NotFound)
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    async fn audit_log(
        &self,
        task_id: TaskId,
        limit: i64,
        offset: i64,
    ) -> Result<iron_defer_domain::ListAuditLogResult, TaskError> {
        if !self.audit_log {
            return Ok(iron_defer_domain::ListAuditLogResult {
                entries: Vec::new(),
                total: 0,
            });
        }

        #[derive(sqlx::FromRow)]
        struct AuditRow {
            id: i64,
            task_id: Uuid,
            from_status: Option<String>,
            to_status: String,
            timestamp: DateTime<Utc>,
            worker_id: Option<Uuid>,
            trace_id: Option<String>,
            metadata: Option<serde_json::Value>,
            total_count: Option<i64>,
        }

        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT id, task_id, from_status, to_status, timestamp, worker_id, trace_id, metadata, COUNT(*) OVER() as total_count \
             FROM task_audit_log WHERE task_id = $1 ORDER BY timestamp ASC, id ASC \
             LIMIT $2 OFFSET $3",
        )
        .bind(task_id.as_uuid())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(PostgresAdapterError::from)?;

        let total = rows.first().and_then(|r| r.total_count).unwrap_or(0) as u64;

        let entries = rows
            .into_iter()
            .map(|r| {
                let from_status = r
                    .from_status
                    .map(|s| parse_status(&s))
                    .transpose()
                    .map_err(|e| PostgresAdapterError::Mapping {
                        reason: format!("invalid from_status in audit log: {e}"),
                    })?;
                let to_status =
                    parse_status(&r.to_status).map_err(|e| PostgresAdapterError::Mapping {
                        reason: format!("invalid to_status in audit log: {e}"),
                    })?;

                Ok(AuditLogEntry::builder()
                    .id(r.id)
                    .task_id(TaskId::from_uuid(r.task_id))
                    .maybe_from_status(from_status)
                    .to_status(to_status)
                    .timestamp(r.timestamp)
                    .maybe_worker_id(r.worker_id.map(WorkerId::from_uuid))
                    .maybe_trace_id(r.trace_id)
                    .maybe_metadata(r.metadata)
                    .build())
            })
            .collect::<Result<Vec<_>, PostgresAdapterError>>()
            .map_err(TaskError::from)?;

        Ok(iron_defer_domain::ListAuditLogResult { entries, total })
    }

    #[instrument(skip(self), err)]
    async fn worker_status(&self) -> Result<Vec<WorkerStatus>, TaskError> {
        #[derive(sqlx::FromRow)]
        struct WorkerRow {
            worker_id: Uuid,
            queue: String,
            tasks_in_flight: i64,
        }

        let rows: Vec<WorkerRow> = sqlx::query_as(
            "SELECT \
                 claimed_by AS worker_id, \
                 queue, \
                 COUNT(*) AS tasks_in_flight \
             FROM tasks \
             WHERE status = 'running' AND claimed_by IS NOT NULL \
             GROUP BY claimed_by, queue \
             ORDER BY queue, claimed_by",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(PostgresAdapterError::from)?;

        let statuses = rows
            .into_iter()
            .map(|row| {
                let queue = QueueName::try_from(row.queue).map_err(|e: ValidationError| {
                    PostgresAdapterError::Mapping {
                        reason: format!("invalid queue name in worker status: {e}"),
                    }
                })?;
                #[allow(clippy::cast_sign_loss)]
                Ok(WorkerStatus {
                    worker_id: WorkerId::from_uuid(row.worker_id),
                    queue,
                    tasks_in_flight: row.tasks_in_flight as u64,
                })
            })
            .collect::<Result<Vec<_>, PostgresAdapterError>>()
            .map_err(TaskError::from)?;

        Ok(statuses)
    }

    #[instrument(skip(self), fields(worker_id = %worker_id), err)]
    async fn release_leases_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<(TaskId, Option<String>)>, TaskError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let rows = sqlx::query!(
            r#"
            UPDATE tasks
            SET status = 'pending',
                claimed_by = NULL,
                claimed_until = NULL,
                scheduled_at = now(),
                updated_at = now(),
                attempts = attempts + 1
            WHERE claimed_by = $1
              AND status = 'running'
            RETURNING id, trace_id
            "#,
            worker_id.as_uuid(),
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        let metadata = serde_json::json!({"reason": "lease released: graceful shutdown"});
        for row in &rows {
            self.insert_audit_row(
                &mut tx,
                row.id,
                Some("running"),
                "pending",
                Some(*worker_id.as_uuid()),
                row.trace_id.as_deref(),
                Some(metadata.clone()),
                None,
            )
            .await?;
        }

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        Ok(rows
            .into_iter()
            .map(|r| (TaskId::from_uuid(r.id), r.trace_id))
            .collect())
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    async fn release_lease_for_task(&self, task_id: TaskId) -> Result<Option<String>, TaskError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query!(
            r#"
            UPDATE tasks
            SET status = 'pending',
                claimed_by = NULL,
                claimed_until = NULL,
                scheduled_at = now(),
                updated_at = now(),
                attempts = attempts + 1
            WHERE id = $1
              AND status = 'running'
            RETURNING trace_id
            "#,
            task_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match row {
            Some(r) => {
                let metadata = serde_json::json!({"reason": "lease released: graceful shutdown"});
                self.insert_audit_row(
                    &mut tx,
                    *task_id.as_uuid(),
                    Some("running"),
                    "pending",
                    None,
                    r.trace_id.as_deref(),
                    Some(metadata),
                    None,
                )
                .await?;
                tx.commit().await.map_err(PostgresAdapterError::from)?;
                Ok(r.trace_id)
            }
            None => Err(TaskError::NotInExpectedState {
                task_id,
                expected: "Running",
            }),
        }
    }

    #[instrument(skip(self), fields(task_id = %task_id), err)]
    async fn suspend(&self, task_id: TaskId) -> Result<TaskRecord, TaskError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            UPDATE tasks
            SET status = 'suspended',
                claimed_by = NULL,
                claimed_until = NULL,
                suspended_at = now(),
                updated_at = now()
            WHERE id = $1
              AND status = 'running'
            RETURNING *
            "#,
            task_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match row {
            Some(r) => {
                self.insert_audit_row(
                    &mut tx,
                    r.id,
                    Some("running"),
                    "suspended",
                    r.claimed_by,
                    r.trace_id.as_deref(),
                    None,
                    r.region.as_deref(),
                )
                .await?;
                tx.commit().await.map_err(PostgresAdapterError::from)?;
                Ok(TaskRecord::try_from(r)?)
            }
            None => Err(TaskError::NotInExpectedState {
                task_id,
                expected: "Running",
            }),
        }
    }

    #[instrument(skip(self, payload), fields(task_id = %task_id), err)]
    async fn signal(
        &self,
        task_id: TaskId,
        payload: Option<serde_json::Value>,
    ) -> Result<TaskRecord, TaskError> {
        if let Some(ref p) = payload {
            let bytes = serde_json::to_vec(p).map_err(|e| TaskError::InvalidPayload {
                kind: iron_defer_domain::PayloadErrorKind::Serialization {
                    message: e.to_string(),
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

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let row = sqlx::query_as!(
            TaskRow,
            r#"
            UPDATE tasks
            SET status = 'pending',
                signal_payload = $1,
                suspended_at = NULL,
                updated_at = now()
            WHERE id = $2
              AND status = 'suspended'
            RETURNING *
            "#,
            payload,
            task_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match row {
            Some(r) => {
                self.insert_audit_row(
                    &mut tx,
                    r.id,
                    Some("suspended"),
                    "pending",
                    None,
                    r.trace_id.as_deref(),
                    None,
                    r.region.as_deref(),
                )
                .await?;
                tx.commit().await.map_err(PostgresAdapterError::from)?;
                Ok(TaskRecord::try_from(r)?)
            }
            None => {
                // Check if it's 404 (not exists) or 409 (exists but not suspended)
                let exists =
                    sqlx::query_scalar!("SELECT 1 FROM tasks WHERE id = $1", task_id.as_uuid())
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(PostgresAdapterError::from)?;

                if exists.is_none() {
                    Err(TaskError::NotFound { id: task_id })
                } else {
                    Err(TaskError::NotInExpectedState {
                        task_id,
                        expected: "Suspended",
                    })
                }
            }
        }
    }

    #[instrument(skip(self), err)]
    async fn expire_suspended_tasks(
        &self,
        suspend_timeout: Duration,
    ) -> Result<Vec<(TaskId, QueueName)>, TaskError> {
        let timeout_secs = suspend_timeout.as_secs();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(PostgresAdapterError::from)?;

        let rows = sqlx::query!(
            r#"
            UPDATE tasks
            SET status = 'failed',
                last_error = 'suspend timeout exceeded',
                updated_at = now()
            WHERE status = 'suspended'
              AND COALESCE(suspended_at, updated_at) < now() - make_interval(secs => $1)
            RETURNING id, queue, trace_id
            "#,
            timeout_secs as f64,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        for row in &rows {
            let metadata = serde_json::json!({"error": "suspend timeout exceeded"});
            self.insert_audit_row(
                &mut tx,
                row.id,
                Some("suspended"),
                "failed",
                None,
                row.trace_id.as_deref(),
                Some(metadata),
                None,
            )
            .await?;
        }

        tx.commit().await.map_err(PostgresAdapterError::from)?;

        rows.into_iter()
            .map(|r| {
                let id = TaskId::from_uuid(r.id);
                let queue =
                    QueueName::try_from(r.queue).map_err(|e| PostgresAdapterError::Mapping {
                        reason: format!("invalid queue name in expired task {id}: {e}"),
                    })?;
                Ok((id, queue))
            })
            .collect::<Result<Vec<_>, PostgresAdapterError>>()
            .map_err(TaskError::from)
    }
}

#[async_trait]
impl TransactionalTaskRepository for PostgresTaskRepository {
    #[instrument(
        skip(self, tx, task),
        fields(
            task_id = %task.id(),
            queue = %task.queue(),
            kind = %task.kind(),
            db.transaction_id = Empty
        ),
        err
    )]
    async fn save_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        task: &TaskRecord,
    ) -> Result<TaskRecord, TaskError> {
        Span::current().record("db.transaction_id", format!("{:p}", &**tx));
        let last_error_capped = task.last_error().map(truncate_last_error_borrow);

        let id = task.id();
        let payload = task.payload();
        let status = status_to_str(task.status());
        let priority = task.priority().get();
        let attempts = task.attempts().get();
        let max_attempts = task.max_attempts().get();
        let scheduled_at = task.scheduled_at();
        let claimed_by = task.claimed_by().as_ref().map(|w| *w.as_uuid());
        let claimed_until = task.claimed_until();
        let idempotency_key = task.idempotency_key();
        let idempotency_expires_at = task.idempotency_expires_at();
        let trace_id = task.trace_id();
        let row = sqlx::query_as!(
            TaskRow,
            r#"
            INSERT INTO tasks (
                id, queue, kind, payload, status, priority,
                attempts, max_attempts, last_error,
                scheduled_at, claimed_by, claimed_until,
                idempotency_key, idempotency_expires_at, trace_id,
                checkpoint, suspended_at, signal_payload,
                region
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
            RETURNING *
            "#,
            id.as_uuid(),
            task.queue().as_str(),
            task.kind().as_str(),
            payload,
            status,
            priority,
            attempts,
            max_attempts,
            last_error_capped,
            scheduled_at,
            claimed_by,
            claimed_until,
            idempotency_key,
            idempotency_expires_at,
            trace_id,
            task.checkpoint(),
            task.suspended_at(),
            task.signal_payload(),
            task.region(),
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        self.insert_audit_row(
            tx,
            row.id,
            None,
            "pending",
            None,
            row.trace_id.as_deref(),
            None,
            row.region.as_deref(),
        )
        .await?;

        let record = TaskRecord::try_from(row)?;
        Ok(record)
    }

    #[instrument(
        skip(self, tx, task),
        fields(
            task_id = %task.id(),
            queue = %task.queue(),
            idempotency_key = ?task.idempotency_key(),
            db.transaction_id = Empty
        ),
        err
    )]
    async fn save_idempotent_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        task: &TaskRecord,
    ) -> Result<(TaskRecord, bool), TaskError> {
        Span::current().record("db.transaction_id", format!("{:p}", &**tx));
        let last_error_capped = task.last_error().map(truncate_last_error_borrow);

        let id = task.id();
        let payload = task.payload();
        let status = status_to_str(task.status());
        let priority = task.priority().get();
        let attempts = task.attempts().get();
        let max_attempts = task.max_attempts().get();
        let scheduled_at = task.scheduled_at();
        let claimed_by = task.claimed_by().as_ref().map(|w| *w.as_uuid());
        let claimed_until = task.claimed_until();
        let idempotency_key = task.idempotency_key();
        let idempotency_expires_at = task.idempotency_expires_at();
        let trace_id = task.trace_id();

        let inserted_row = sqlx::query_as!(
            TaskRow,
            r#"
            INSERT INTO tasks (
                id, queue, kind, payload, status, priority,
                attempts, max_attempts, last_error,
                scheduled_at, claimed_by, claimed_until,
                idempotency_key, idempotency_expires_at, trace_id,
                checkpoint, suspended_at, signal_payload,
                region
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
            ON CONFLICT (queue, idempotency_key)
                WHERE idempotency_key IS NOT NULL
                  AND status NOT IN ('completed', 'failed', 'cancelled')
            DO NOTHING
            RETURNING *
            "#,
            id.as_uuid(),
            task.queue().as_str(),
            task.kind().as_str(),
            payload,
            status,
            priority,
            attempts,
            max_attempts,
            last_error_capped,
            scheduled_at,
            claimed_by,
            claimed_until,
            idempotency_key,
            idempotency_expires_at,
            trace_id,
            task.checkpoint(),
            task.suspended_at(),
            task.signal_payload(),
            task.region(),
        )
        .fetch_optional(&mut **tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        if let Some(row) = inserted_row {
            self.insert_audit_row(
                tx,
                row.id,
                None,
                "pending",
                None,
                row.trace_id.as_deref(),
                None,
                row.region.as_deref(),
            )
            .await?;
            let record = TaskRecord::try_from(row)?;
            return Ok((record, true));
        }

        let existing_row = sqlx::query_as!(
            TaskRow,
            r#"
            SELECT * FROM tasks
            WHERE queue = $1 AND idempotency_key = $2
              AND status NOT IN ('completed', 'failed', 'cancelled')
            "#,
            task.queue().as_str(),
            idempotency_key,
        )
        .fetch_optional(&mut **tx)
        .await
        .map_err(PostgresAdapterError::from)?;

        match existing_row {
            Some(row) => {
                let record = TaskRecord::try_from(row)?;
                Ok((record, false))
            }
            None => Err(TaskError::Storage {
                source: format!(
                    "idempotency conflict detected for queue={} key={}, but no non-terminal task found (likely transitioned to terminal state during race)",
                    task.queue(),
                    idempotency_key.unwrap_or("none")
                ).into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_below_limit_is_unchanged() {
        let s = "hello world".to_string();
        assert_eq!(truncate_last_error(s.clone()), s);
    }

    #[test]
    fn truncate_at_limit_is_unchanged() {
        let s = "a".repeat(LAST_ERROR_MAX_BYTES);
        let out = truncate_last_error(s);
        assert_eq!(out.len(), LAST_ERROR_MAX_BYTES);
    }

    #[test]
    fn truncate_above_limit_caps_at_max_bytes() {
        let s = "a".repeat(LAST_ERROR_MAX_BYTES * 3);
        let out = truncate_last_error(s);
        assert_eq!(out.len(), LAST_ERROR_MAX_BYTES);
    }

    #[test]
    fn truncate_borrow_under_limit_returns_full_slice() {
        let s = "hello";
        assert_eq!(truncate_last_error_borrow(s), s);
        assert!(std::ptr::eq(truncate_last_error_borrow(s), s));
    }

    #[test]
    fn truncate_borrow_over_limit_caps_at_max_bytes() {
        let s = "x".repeat(LAST_ERROR_MAX_BYTES * 3);
        let out = truncate_last_error_borrow(&s);
        assert_eq!(out.len(), LAST_ERROR_MAX_BYTES);
    }

    #[test]
    fn truncate_borrow_preserves_utf8_boundary() {
        let prefix = "a".repeat(LAST_ERROR_MAX_BYTES - 1);
        let s = format!("{prefix}é{}", "z".repeat(100));
        let out = truncate_last_error_borrow(&s);
        assert_eq!(out.len(), LAST_ERROR_MAX_BYTES - 1);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_preserves_utf8_boundary() {
        // Build a string that pushes a multi-byte char across the cap.
        // "héllo" → 'é' is 2 bytes (0xC3 0xA9).
        let prefix = "a".repeat(LAST_ERROR_MAX_BYTES - 1);
        let s = format!("{prefix}é{}", "z".repeat(100));
        let out = truncate_last_error(s);
        // Cap is mid-'é', so floor_char_boundary backs up 1 byte.
        assert_eq!(out.len(), LAST_ERROR_MAX_BYTES - 1);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn parse_status_round_trips_all_variants() {
        for variant in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Suspended,
        ] {
            let s = status_to_str(variant);
            assert_eq!(parse_status(s).unwrap(), variant);
        }
    }

    #[test]
    fn parse_status_rejects_unknown() {
        let err = parse_status("bogus").unwrap_err();
        match err {
            PostgresAdapterError::Mapping { reason } => {
                assert!(reason.contains("unknown task status"));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn try_from_rejects_empty_kind() {
        let row = sample_row(|r| r.kind = String::new());
        let err = TaskRecord::try_from(row).unwrap_err();
        match err {
            PostgresAdapterError::Mapping { reason } => {
                assert!(reason.contains("kind must not be empty"));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn try_from_rejects_negative_attempts() {
        let row = sample_row(|r| r.attempts = -1);
        let err = TaskRecord::try_from(row).unwrap_err();
        assert!(matches!(err, PostgresAdapterError::Mapping { .. }));
    }

    #[test]
    fn try_from_rejects_negative_max_attempts() {
        let row = sample_row(|r| r.max_attempts = -5);
        let err = TaskRecord::try_from(row).unwrap_err();
        assert!(matches!(err, PostgresAdapterError::Mapping { .. }));
    }

    #[test]
    fn try_from_rejects_invalid_queue_name() {
        let row = sample_row(|r| r.queue = String::new());
        let err = TaskRecord::try_from(row).unwrap_err();
        assert!(matches!(err, PostgresAdapterError::Mapping { .. }));
    }

    #[test]
    fn try_from_rejects_unknown_status() {
        let row = sample_row(|r| r.status = "weird".to_string());
        let err = TaskRecord::try_from(row).unwrap_err();
        assert!(matches!(err, PostgresAdapterError::Mapping { .. }));
    }

    #[test]
    fn try_from_truncates_oversized_last_error() {
        let row = sample_row(|r| {
            r.last_error = Some("e".repeat(LAST_ERROR_MAX_BYTES * 2));
        });
        let record = TaskRecord::try_from(row).unwrap();
        assert_eq!(record.last_error().unwrap().len(), LAST_ERROR_MAX_BYTES);
    }

    #[test]
    fn try_from_round_trips_valid_row() {
        let row = sample_row(|_| {});
        let record = TaskRecord::try_from(row).unwrap();
        assert_eq!(*record.kind(), "test_kind");
        assert_eq!(record.queue().as_str(), "default");
        assert_eq!(record.status(), TaskStatus::Pending);
    }

    fn sample_row<F: FnOnce(&mut TaskRow)>(mutate: F) -> TaskRow {
        let now = Utc::now();
        let mut row = TaskRow {
            id: Uuid::new_v4(),
            queue: "default".to_string(),
            kind: "test_kind".to_string(),
            payload: serde_json::json!({}),
            status: "pending".to_string(),
            priority: 0,
            attempts: 0,
            max_attempts: 3,
            last_error: None,
            scheduled_at: now,
            claimed_by: None,
            claimed_until: None,
            created_at: now,
            updated_at: now,
            idempotency_key: None,
            idempotency_expires_at: None,
            trace_id: None,
            checkpoint: None,
            suspended_at: None,
            signal_payload: None,
            region: None,
        };
        mutate(&mut row);
        row
    }
}
