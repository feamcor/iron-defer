//! iron-defer domain crate.
//!
//! Pure domain types for the iron-defer task engine. No infrastructure or
//! framework dependencies. Per the architecture rule "no logic in lib.rs",
//! this file only re-exports public items.

#![forbid(unsafe_code)]

pub mod error;
pub mod model;

pub use crate::error::{
    ClaimError, ExecutionErrorKind, PayloadErrorKind, TaskError, ValidationError,
};
pub use crate::model::{
    AttemptCount, AuditLogEntry, CancelResult, CheckpointWriter, ListAuditLogResult, ListTasksFilter,
    ListTasksResult, MaxAttempts, Priority, QueueName, QueueStatistics, Task, TaskContext, TaskId,
    TaskKind, TaskRecord, TaskStatus, WorkerId, WorkerStatus, SIGNAL_PAYLOAD_MAX_BYTES,
};
