//! Typed domain error enums per ADR-0002.
//!
//! One enum per concept; no `Box<dyn Error>` *except* for `TaskError::Storage`
//! and `TaskError::Migration`, which carry a boxed source so adapters can
//! preserve the underlying error chain without forcing the domain crate to
//! depend on infrastructure crates. Adapters in the infrastructure layer
//! translate their crate-specific errors to these types at the layer
//! boundary via explicit `From` impls.

use thiserror::Error;

use crate::model::{TaskId, WorkerId};

/// Structured source for `TaskError::InvalidPayload` (CR10).
#[derive(Debug, Error)]
pub enum PayloadErrorKind {
    #[error("deserialization failed: {message}")]
    Deserialization { message: String },

    /// Serialization errors are rare since we're serializing an in-memory
    /// object that was already deserialized. Kept for API completeness
    /// in case Task implementations have fallible Serialize impls.
    #[error("serialization failed: {message}")]
    Serialization { message: String },

    #[error("{message}")]
    Validation { message: String },

    #[error("engine already started")]
    AlreadyStarted,
}

/// Structured source for `TaskError::ExecutionFailed` (CR10).
#[derive(Debug, Error)]
pub enum ExecutionErrorKind {
    #[error("handler failed: {source}")]
    HandlerFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("handler panicked: {message}")]
    HandlerPanicked { message: String },

    #[error("no handler registered for kind: {kind}")]
    MissingHandler { kind: String },
}

/// Errors raised by task lifecycle and persistence operations.
#[derive(Debug, Error)]
pub enum TaskError {
    #[error("task not found: {id}")]
    NotFound { id: TaskId },

    #[error("task {id} is already claimed by worker {worker_id}")]
    AlreadyClaimed { id: TaskId, worker_id: WorkerId },

    #[error("task {task_id} is not in {expected} status")]
    NotInExpectedState {
        task_id: TaskId,
        expected: &'static str,
    },

    #[error("task payload is invalid: {kind}")]
    InvalidPayload { kind: PayloadErrorKind },

    #[error("task execution failed: {kind}")]
    ExecutionFailed { kind: ExecutionErrorKind },

    /// Persistence backend reported a failure. The boxed source preserves the
    /// underlying error chain (typically `PostgresAdapterError` wrapping
    /// `sqlx::Error`) so `tracing` `err` fields capture the full causality.
    #[error("task storage error: {source}")]
    Storage {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Database migration failed. Separate from `Storage` so callers can
    /// programmatically distinguish migration errors from runtime DB errors.
    #[error("database migration failed: {source}")]
    Migration {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The task handler requests suspension (G7 HITL). This is NOT an error —
    /// it is a control-flow signal from `ctx.suspend()` that the worker
    /// dispatch loop intercepts to transition the task to `Suspended` status.
    #[error("task suspend requested")]
    SuspendRequested,
}

/// Errors raised by the atomic claiming protocol.
#[derive(Debug, Error)]
pub enum ClaimError {
    #[error("no eligible task to claim in queue {queue}")]
    NoneAvailable { queue: String },

    #[error("claim aborted by storage: {reason}")]
    Storage { reason: String },
}

impl From<ClaimError> for TaskError {
    fn from(err: ClaimError) -> Self {
        match err {
            ClaimError::NoneAvailable { queue } => TaskError::InvalidPayload {
                kind: PayloadErrorKind::Validation {
                    message: format!("no eligible task to claim in queue {queue}"),
                },
            },
            ClaimError::Storage { reason } => TaskError::Storage {
                source: reason.into(),
            },
        }
    }
}

/// Errors raised when constructing validated value objects.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("queue name must not be empty")]
    EmptyQueueName,

    #[error("queue name must not contain whitespace: {value:?}")]
    QueueNameWhitespace { value: String },

    #[error("queue name must not contain control or zero-width characters: {value:?}")]
    QueueNameForbiddenChar { value: String },

    #[error("queue name length {length} exceeds maximum {max}")]
    QueueNameTooLong { length: usize, max: usize },

    #[error("task kind must not be empty")]
    EmptyTaskKind,

    #[error("attempt count must be >= 0, got {value}")]
    NegativeAttemptCount { value: i32 },

    #[error("max_attempts must be >= 1, got {value}")]
    InvalidMaxAttempts { value: i32 },

    #[error("payload size {actual_bytes} exceeds maximum {max_bytes} bytes")]
    PayloadTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
}
