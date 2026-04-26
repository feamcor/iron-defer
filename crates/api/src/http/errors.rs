//! `AppError` — maps domain errors to HTTP responses.
//!
//! Architecture references:
//! - §Format Patterns (REST API): error response shape and HTTP status mapping
//! - §Format Patterns: error codes are `SCREAMING_SNAKE_CASE`
//!
//! Every error response uses the shape:
//! ```json
//! { "error": { "code": "SCREAMING_SNAKE_CASE", "message": "..." } }
//! ```

use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use iron_defer_domain::TaskError;
use serde::Serialize;
use utoipa::ToSchema;

/// Structured error response body per Architecture §Format Patterns (REST API).
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

/// Inner error detail.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

/// Application-level error that converts to an axum HTTP response.
#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    code: String,
    message: String,
}

impl AppError {
    /// 404 Not Found.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "TASK_NOT_FOUND".to_string(),
            message: message.into(),
        }
    }

    /// 409 Conflict — task is already claimed by a worker.
    pub fn already_claimed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "TASK_ALREADY_CLAIMED".to_string(),
            message: message.into(),
        }
    }

    /// 422 Unprocessable Entity — invalid query parameter value.
    pub fn invalid_query_parameter(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "INVALID_QUERY_PARAMETER".to_string(),
            message: message.into(),
        }
    }

    /// 500 Internal Server Error — unexpected server-side condition.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR".to_string(),
            message: message.into(),
        }
    }

    /// 409 Conflict — task is in a terminal state (completed, failed, cancelled).
    pub fn terminal_state(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "TASK_IN_TERMINAL_STATE".to_string(),
            message: message.into(),
        }
    }

    /// 409 Conflict — task is suspended (G7 HITL).
    pub fn task_suspended(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "TASK_SUSPENDED".to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl From<TaskError> for AppError {
    fn from(err: TaskError) -> Self {
        match &err {
            TaskError::NotFound { .. } => Self {
                status: StatusCode::NOT_FOUND,
                code: "TASK_NOT_FOUND".to_string(),
                message: err.to_string(),
            },
            TaskError::InvalidPayload { .. } => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "INVALID_PAYLOAD".to_string(),
                message: err.to_string(),
            },
            TaskError::AlreadyClaimed { .. } => Self {
                status: StatusCode::CONFLICT,
                code: "TASK_ALREADY_CLAIMED".to_string(),
                message: err.to_string(),
            },
            TaskError::NotInExpectedState { task_id, expected } => Self {
                status: StatusCode::CONFLICT,
                code: "TASK_NOT_IN_EXPECTED_STATE".to_string(),
                message: format!("task {task_id} is not in {expected} status"),
            },
            TaskError::SuspendRequested => Self {
                status: StatusCode::CONFLICT,
                code: "TASK_NOT_IN_EXPECTED_STATE".to_string(),
                message: "task suspend requested".to_string(),
            },
            TaskError::ExecutionFailed { .. }
            | TaskError::Storage { .. }
            | TaskError::Migration { .. } => {
                tracing::error!(error = %err, "internal error processing request");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    code: "INTERNAL_ERROR".to_string(),
                    message: "internal server error".to_string(),
                }
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
            },
        };
        (self.status, axum::Json(body)).into_response()
    }
}
