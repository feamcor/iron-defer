//! Task HTTP handlers — `POST /tasks`, `GET /tasks/{id}`, and `DELETE /tasks/{id}`.
//!
//! Architecture references:
//! - §Requirements to Structure Mapping: handler → `SchedulerService` mapping
//! - §Integration Points: enqueue data flow
//! - ADR-0006 / §Enforcement Guidelines: camelCase JSON field naming

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use iron_defer_domain::{
    CancelResult, ListTasksFilter, QueueName, TaskError, TaskId, TaskRecord, TaskStatus,
};
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::IronDefer;
use crate::http::errors::{AppError, ErrorResponse};
use crate::http::extractors::{JsonBody, PathParam};

const DEFAULT_LIST_LIMIT: u32 = 50;
const MAX_LIST_LIMIT: u32 = 100;
const MAX_OFFSET: u32 = 10_000;

// ---------------------------------------------------------------------------
// Request / Response DTOs
// ---------------------------------------------------------------------------

/// `POST /tasks` request body. Architecture §C5: single endpoint,
/// `queue` defaults to `"default"`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    /// Queue name. Defaults to `"default"` if omitted.
    pub queue: Option<String>,
    /// Task kind discriminator — must match a registered handler.
    pub kind: String,
    /// Arbitrary JSON payload for the task handler.
    pub payload: serde_json::Value,
    /// Optional future execution time (ISO 8601 UTC).
    pub scheduled_at: Option<DateTime<Utc>>,
    /// Optional priority override (higher = picked sooner). Default: 0.
    pub priority: Option<i16>,
    /// Optional max retry attempts override. Default: 3.
    pub max_attempts: Option<i32>,
    /// Optional idempotency key for exactly-once submission. Keys are scoped per-queue.
    pub idempotency_key: Option<String>,
    /// Optional region label for geographic worker pinning.
    pub region: Option<String>,
}

/// Task record response DTO with camelCase JSON naming (ADR-0006).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
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
    /// Optional idempotency key used during submission.
    pub idempotency_key: Option<String>,
    /// Timestamp after which the idempotency key is released for reuse.
    pub idempotency_expires_at: Option<DateTime<Utc>>,
    /// W3C trace ID propagated from enqueue to execution.
    pub trace_id: Option<String>,
    /// Last checkpoint data persisted by the task handler (G6).
    pub last_checkpoint: Option<serde_json::Value>,
    /// Timestamp when the task was suspended (G7 HITL).
    pub suspended_at: Option<DateTime<Utc>>,
    /// Signal payload attached when the task was suspended (G7 HITL).
    pub signal_payload: Option<serde_json::Value>,
    /// Region label for geographic worker pinning (G8).
    pub region: Option<String>,
}

impl From<TaskRecord> for TaskResponse {
    fn from(mut r: TaskRecord) -> Self {
        let id = *r.id().as_uuid();
        let queue = r.queue().to_string();
        let kind = r.kind().to_string();
        let status = status_to_str(r.status());
        let priority = r.priority().get();
        let attempts = r.attempts().get();
        let max_attempts = r.max_attempts().get();
        let last_error = r.last_error().map(str::to_owned);
        let scheduled_at = r.scheduled_at();
        let claimed_by = r.claimed_by().map(|w| *w.as_uuid());
        let claimed_until = r.claimed_until();
        let created_at = r.created_at();
        let updated_at = r.updated_at();
        let idempotency_key = r.idempotency_key().map(str::to_owned);
        let idempotency_expires_at = r.idempotency_expires_at();
        let trace_id = r.trace_id().map(str::to_owned);
        let last_checkpoint = r.take_checkpoint().map(Arc::unwrap_or_clone);
        let suspended_at = r.suspended_at();
        let signal_payload = r.take_signal_payload().map(Arc::unwrap_or_clone);
        let region = r.region().map(str::to_owned);
        let payload = Arc::unwrap_or_clone(r.take_payload());
        Self {
            id,
            queue,
            kind,
            payload,
            status,
            priority,
            attempts,
            max_attempts,
            last_error,
            scheduled_at,
            claimed_by,
            claimed_until,
            created_at,
            updated_at,
            idempotency_key,
            idempotency_expires_at,
            trace_id,
            last_checkpoint,
            suspended_at,
            signal_payload,
            region,
        }
    }
}

/// Map `TaskStatus` to its lowercase string representation for JSON output.
fn status_to_str(s: TaskStatus) -> String {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Suspended => "suspended",
        _ => "unknown",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /tasks` — submit a new task for asynchronous execution.
///
/// # Errors
///
/// Returns `AppError` (422) if the request body fails validation or
/// no handler is registered for the given `kind`.
#[utoipa::path(
    post, path = "/tasks",
    request_body = CreateTaskRequest,
    responses(
        (status = 201, description = "Task created", body = TaskResponse),
        (status = 200, description = "Duplicate idempotency key — existing task returned", body = TaskResponse),
        (status = 422, description = "Invalid input", body = ErrorResponse),
    )
)]
#[instrument(skip_all, fields(method = "POST", path = "/tasks"), err)]
pub async fn create_task(
    State(engine): State<Arc<IronDefer>>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskResponse>), AppError> {
    let queue = body.queue.as_deref().unwrap_or("default");

    let trace_id = extract_trace_id(&headers);
    let region = body.region.as_deref();

    if let Some(ref idempotency_key) = body.idempotency_key {
        let (record, created) = engine
            .enqueue_raw_idempotent(
                queue,
                &body.kind,
                body.payload,
                body.scheduled_at,
                body.priority,
                body.max_attempts,
                idempotency_key,
                trace_id.as_deref(),
                region,
            )
            .await?;
        let status = if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        return Ok((status, Json(TaskResponse::from(record))));
    }

    let record = engine
        .enqueue_raw(
            queue,
            &body.kind,
            body.payload,
            body.scheduled_at,
            body.priority,
            body.max_attempts,
            trace_id.as_deref(),
            region,
        )
        .await?;

    Ok((StatusCode::CREATED, Json(TaskResponse::from(record))))
}

fn extract_trace_id(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("traceparent")?.to_str().ok()?;
    let mut parts = value.split('-');

    // Strict W3C validation. Format: {version}-{trace_id}-{span_id}-{flags}
    if parts.next()? != "00" {
        return None;
    }

    let trace_id = parts.next()?;
    if trace_id.len() != 32 || !trace_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let span_id = parts.next()?;
    if span_id.len() != 16 || !span_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let flags = parts.next()?;
    if flags.len() != 2 || !flags.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    if parts.next().is_some() {
        return None;
    }

    Some(trace_id.to_owned())
}

/// `GET /tasks/{id}` — retrieve a task by UUID.
///
/// # Errors
///
/// Returns `AppError` (404) if the task does not exist, or (500) on
/// storage failure.
#[utoipa::path(
    get, path = "/tasks/{id}",
    params(("id" = Uuid, Path, description = "Task UUID")),
    responses(
        (status = 200, description = "Task record", body = TaskResponse),
        (status = 404, description = "Task not found", body = ErrorResponse),
    )
)]
#[instrument(skip_all, fields(method = "GET", path = "/tasks/{id}"), err)]
pub async fn get_task(
    State(engine): State<Arc<IronDefer>>,
    PathParam(id): PathParam<Uuid>,
) -> Result<Json<TaskResponse>, AppError> {
    let task_id = TaskId::from_uuid(id);

    let record = engine
        .find(task_id)
        .await?
        .ok_or_else(|| TaskError::NotFound { id: task_id })?;

    Ok(Json(TaskResponse::from(record)))
}

/// `DELETE /tasks/{id}` — cancel a pending task.
///
/// # Errors
///
/// Returns `AppError` (404) if the task does not exist, (409) if the task
/// is not in a cancellable state, or (500) on storage failure.
#[utoipa::path(
    delete, path = "/tasks/{id}",
    params(("id" = Uuid, Path, description = "Task UUID")),
    responses(
        (status = 200, description = "Task cancelled", body = TaskResponse),
        (status = 404, description = "Task not found", body = ErrorResponse),
        (status = 409, description = "Task not cancellable", body = ErrorResponse),
    )
)]
#[instrument(skip_all, fields(method = "DELETE", path = "/tasks/{id}"), err)]
pub async fn delete_task(
    State(engine): State<Arc<IronDefer>>,
    PathParam(id): PathParam<Uuid>,
) -> Result<Json<TaskResponse>, AppError> {
    let task_id = TaskId::from_uuid(id);

    match engine.cancel(task_id).await? {
        CancelResult::Cancelled(record) => {
            iron_defer_application::emit_otel_state_transition(
                record.trace_id(),
                record.id(),
                "pending",
                "cancelled",
                record.queue().as_str(),
                record.kind().as_str(),
                None, // manual cancellation - no specific worker
                record.attempts().get(),
            );
            Ok(Json(TaskResponse::from(record)))
        }
        CancelResult::NotFound => Err(TaskError::NotFound { id: task_id }.into()),
        CancelResult::NotCancellable { current_status } => match current_status {
            // Under concurrent cancellation the CTE's UPDATE blocks on the
            // row lock held by the winning transaction.  When the winner
            // commits (pending → cancelled), the loser re-evaluates the WHERE
            // clause and finds no match — correct.  However the outer SELECT
            // still uses the statement-level snapshot taken *before* the
            // winner committed, so `original_status` reads as 'pending'.
            // This is normal READ COMMITTED behaviour, not an invariant
            // violation — return 409 like any other non-cancellable state.
            TaskStatus::Pending => Err(AppError::terminal_state(format!(
                "task {id} was concurrently modified and is no longer cancellable"
            ))),
            TaskStatus::Running => Err(AppError::already_claimed(format!(
                "task {id} is currently running and cannot be cancelled"
            ))),
            TaskStatus::Suspended => {
                Err(AppError::task_suspended(format!("task {id} is suspended")))
            }
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                Err(AppError::terminal_state(format!(
                    "task {id} is in terminal state '{}'",
                    status_to_str(current_status)
                )))
            }
            _ => {
                tracing::error!(
                    task_id = %id,
                    status = ?current_status,
                    "cancel returned unrecognized task status"
                );
                Err(AppError::internal("internal server error"))
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Signal — POST /tasks/{id}/signal
// ---------------------------------------------------------------------------

/// `POST /tasks/{id}/signal` request body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalTaskRequest {
    pub payload: Option<serde_json::Value>,
}

/// `POST /tasks/{id}/signal` — resume a suspended task with an optional signal payload.
///
/// # Errors
///
/// Returns `AppError` (404) if the task does not exist, (409) if the task
/// is not in `Suspended` status, or (500) on storage failure.
#[utoipa::path(
    post, path = "/tasks/{id}/signal",
    params(("id" = Uuid, Path, description = "Task UUID")),
    request_body = SignalTaskRequest,
    responses(
        (status = 200, description = "Task resumed", body = TaskResponse),
        (status = 404, description = "Task not found", body = ErrorResponse),
        (status = 409, description = "Task not in Suspended status", body = ErrorResponse),
    )
)]
#[instrument(skip_all, fields(method = "POST", path = "/tasks/{id}/signal"), err)]
pub async fn signal_task(
    State(engine): State<Arc<IronDefer>>,
    PathParam(id): PathParam<Uuid>,
    JsonBody(body): JsonBody<SignalTaskRequest>,
) -> Result<Json<TaskResponse>, AppError> {
    let task_id = TaskId::from_uuid(id);
    let record = engine.signal(task_id, body.payload).await?;
    Ok(Json(TaskResponse::from(record)))
}

// ---------------------------------------------------------------------------
// Audit log — GET /tasks/{id}/audit
// ---------------------------------------------------------------------------

/// Audit log entry response DTO with camelCase JSON naming (ADR-0006).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogResponse {
    pub id: i64,
    pub task_id: Uuid,
    pub from_status: Option<String>,
    pub to_status: String,
    pub timestamp: DateTime<Utc>,
    pub worker_id: Option<Uuid>,
    pub trace_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

impl From<iron_defer_domain::AuditLogEntry> for AuditLogResponse {
    fn from(e: iron_defer_domain::AuditLogEntry) -> Self {
        Self {
            id: e.id(),
            task_id: *e.task_id().as_uuid(),
            from_status: e.from_status().map(|s| s.as_str().to_owned()),
            to_status: e.to_status().as_str().to_owned(),
            timestamp: e.timestamp(),
            worker_id: e.worker_id().map(|w| *w.as_uuid()),
            trace_id: e.trace_id().map(str::to_owned),
            metadata: e.metadata().cloned(),
        }
    }
}

/// Query parameters for `GET /tasks/{id}/audit`.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogParams {
    #[serde(default = "default_audit_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_audit_limit() -> i64 {
    100
}

/// Paginated response for `GET /tasks/{id}/audit`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogListResponse {
    pub entries: Vec<AuditLogResponse>,
    pub total: u64,
    pub limit: i64,
    pub offset: i64,
}

/// `GET /tasks/{id}/audit` — retrieve the audit log for a task.
#[utoipa::path(
    get, path = "/tasks/{id}/audit",
    params(("id" = Uuid, Path, description = "Task UUID"), AuditLogParams),
    responses(
        (status = 200, description = "Audit log entries", body = AuditLogListResponse),
        (status = 404, description = "Task not found", body = ErrorResponse),
    )
)]
#[instrument(skip_all, fields(method = "GET", path = "/tasks/{id}/audit"), err)]
pub async fn get_audit_log(
    State(engine): State<Arc<IronDefer>>,
    PathParam(id): PathParam<Uuid>,
    axum::extract::Query(params): axum::extract::Query<AuditLogParams>,
) -> Result<Json<AuditLogListResponse>, AppError> {
    let task_id = TaskId::from_uuid(id);
    let limit = params.limit.clamp(1, 1000);
    let offset = params.offset.max(0);

    if engine.find(task_id).await?.is_none() {
        return Err(AppError::from(TaskError::NotFound { id: task_id }));
    }

    let result = engine.audit_log(task_id, limit, offset).await?;
    Ok(Json(AuditLogListResponse {
        entries: result
            .entries
            .into_iter()
            .map(AuditLogResponse::from)
            .collect(),
        total: result.total,
        limit,
        offset,
    }))
}

// ---------------------------------------------------------------------------
// List tasks — GET /tasks
// ---------------------------------------------------------------------------

/// Query parameters for `GET /tasks`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListTasksQuery {
    pub queue: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Paginated response for `GET /tasks`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

fn parse_status_filter(s: &str) -> Result<TaskStatus, AppError> {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "pending" => Ok(TaskStatus::Pending),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        "suspended" => Ok(TaskStatus::Suspended),
        other => Err(AppError::invalid_query_parameter(format!(
            "invalid status filter: '{other}'; expected one of: pending, running, completed, failed, cancelled, suspended"
        ))),
    }
}

/// `GET /tasks` — list tasks with optional filters and pagination.
///
/// When no `queue` or `status` filter is provided, the default limit is 100
/// (instead of 50) and a warning is logged. This prevents unbounded queries
/// while still returning useful results.
#[utoipa::path(
    get, path = "/tasks",
    params(ListTasksQuery),
    responses(
        (status = 200, description = "Paginated task list", body = ListTasksResponse),
        (status = 422, description = "Invalid query parameter", body = ErrorResponse),
    )
)]
/// # Errors
///
/// Returns `AppError` for invalid query parameters or storage failures.
#[instrument(skip_all, fields(method = "GET", path = "/tasks"), err)]
pub async fn list_tasks(
    State(engine): State<Arc<IronDefer>>,
    axum::extract::Query(params): axum::extract::Query<ListTasksQuery>,
) -> Result<Json<ListTasksResponse>, AppError> {
    let status = params
        .status
        .as_deref()
        .map(parse_status_filter)
        .transpose()?;

    let queue = params
        .queue
        .as_deref()
        .map(QueueName::try_from)
        .transpose()
        .map_err(|e| AppError::invalid_query_parameter(format!("invalid queue name: {e}")))?;

    let unfiltered = queue.is_none() && status.is_none();
    if unfiltered {
        return Err(AppError::invalid_query_parameter(
            "unfiltered task list is disabled for performance; please provide a 'queue' or 'status' filter",
        ));
    }

    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);

    let provided_offset = params.offset.unwrap_or(0);
    let offset = provided_offset.min(MAX_OFFSET);
    if provided_offset > MAX_OFFSET {
        warn!(
            event = "offset_capped",
            provided_offset,
            capped_offset = MAX_OFFSET,
            "Request offset exceeded maximum allowed; capping to 10,000"
        );
    }

    let filter = ListTasksFilter {
        queue,
        status,
        limit,
        offset,
    };

    let result = engine.list_tasks(&filter).await?;

    Ok(Json(ListTasksResponse {
        tasks: result.tasks.into_iter().map(TaskResponse::from).collect(),
        total: result.total,
        limit,
        offset,
    }))
}
