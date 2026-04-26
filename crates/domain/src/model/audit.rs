use chrono::{DateTime, Utc};

use super::task::{TaskId, TaskStatus};
use super::worker::WorkerId;

#[derive(Debug, Clone, bon::Builder)]
#[non_exhaustive]
pub struct AuditLogEntry {
    id: i64,
    task_id: TaskId,
    from_status: Option<TaskStatus>,
    to_status: TaskStatus,
    timestamp: DateTime<Utc>,
    worker_id: Option<WorkerId>,
    trace_id: Option<String>,
    metadata: Option<serde_json::Value>,
}

impl AuditLogEntry {

    #[must_use]
    pub fn id(&self) -> i64 {
        self.id
    }

    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    #[must_use]
    pub fn from_status(&self) -> Option<TaskStatus> {
        self.from_status
    }

    #[must_use]
    pub fn to_status(&self) -> TaskStatus {
        self.to_status
    }

    #[must_use]
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    #[must_use]
    pub fn worker_id(&self) -> Option<WorkerId> {
        self.worker_id
    }

    #[must_use]
    pub fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref()
    }
}

/// Paginated result of an audit log query.
#[derive(Debug, Clone)]
pub struct ListAuditLogResult {
    pub entries: Vec<AuditLogEntry>,
    pub total: u64,
}
