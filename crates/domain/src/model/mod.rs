//! Domain model module — task, worker, queue, kind, and identifier types.

pub mod attempts;
pub mod audit;
pub mod kind;
pub mod priority;
pub mod queue;
pub mod task;
pub mod worker;

pub use attempts::{AttemptCount, MaxAttempts};
pub use audit::{AuditLogEntry, ListAuditLogResult};
pub use kind::TaskKind;
pub use priority::Priority;
pub use queue::{QueueName, QueueStatistics};
pub use task::{
    CancelResult, CheckpointWriter, IDEMPOTENCY_KEY_MAX_LEN, ListTasksFilter, ListTasksResult,
    REGION_MAX_LEN, SIGNAL_PAYLOAD_MAX_BYTES, Task, TaskContext, TaskId, TaskRecord, TaskStatus,
};
pub use worker::{WorkerId, WorkerStatus};
