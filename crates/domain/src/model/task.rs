//! Task-related domain types: identifiers, status enum, the `Task` trait,
//! task execution context, and the `TaskRecord` persistence shape.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::TaskError;
use crate::model::{
    attempts::{AttemptCount, MaxAttempts},
    kind::TaskKind,
    priority::Priority,
    queue::QueueName,
    worker::WorkerId,
};

/// Stable identifier for a persisted task. Wraps a UUID v4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Generate a fresh random task identifier.
    ///
    /// `Default` is intentionally NOT implemented: identifiers should be
    /// created explicitly so callers do not silently allocate randomness
    /// via `..Default::default()` patterns.
    #[must_use]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Construct a `TaskId` from a pre-existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Lifecycle status of a task in the iron-defer engine.
///
/// String values are lowercase to match the SQL `status` column defined in
/// `docs/artifacts/planning/architecture.md` §D1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Suspended,
}

impl TaskStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Suspended => "suspended",
        }
    }
}

/// Persistence shape of a task. Field names and types mirror the
/// `tasks` table schema in architecture §D1.1.
///
/// Marked `#[non_exhaustive]` because future stories will add fields
/// (e.g. retry-policy metadata) and this type appears in the public API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[non_exhaustive]
pub struct TaskRecord {
    pub(crate) id: TaskId,
    pub(crate) queue: QueueName,
    pub(crate) kind: TaskKind,
    pub(crate) payload: Arc<serde_json::Value>,
    pub(crate) status: TaskStatus,
    pub(crate) priority: Priority,
    pub(crate) attempts: AttemptCount,
    pub(crate) max_attempts: MaxAttempts,
    pub(crate) last_error: Option<String>,
    pub(crate) scheduled_at: DateTime<Utc>,
    pub(crate) claimed_by: Option<WorkerId>,
    pub(crate) claimed_until: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) idempotency_key: Option<String>,
    pub(crate) idempotency_expires_at: Option<DateTime<Utc>>,
    pub(crate) trace_id: Option<String>,
    pub(crate) checkpoint: Option<Arc<serde_json::Value>>,
    pub(crate) suspended_at: Option<DateTime<Utc>>,
    pub(crate) signal_payload: Option<Arc<serde_json::Value>>,
    pub(crate) region: Option<String>,
}

impl TaskRecord {
    #[must_use]
    pub fn id(&self) -> TaskId {
        self.id
    }

    #[must_use]
    pub fn queue(&self) -> &QueueName {
        &self.queue
    }

    #[must_use]
    pub fn kind(&self) -> &TaskKind {
        &self.kind
    }

    #[must_use]
    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    #[must_use]
    pub fn status(&self) -> TaskStatus {
        self.status
    }

    #[must_use]
    pub fn priority(&self) -> Priority {
        self.priority
    }

    #[must_use]
    pub fn attempts(&self) -> AttemptCount {
        self.attempts
    }

    #[must_use]
    pub fn max_attempts(&self) -> MaxAttempts {
        self.max_attempts
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[must_use]
    pub fn scheduled_at(&self) -> DateTime<Utc> {
        self.scheduled_at
    }

    #[must_use]
    pub fn claimed_by(&self) -> Option<WorkerId> {
        self.claimed_by
    }

    #[must_use]
    pub fn claimed_until(&self) -> Option<DateTime<Utc>> {
        self.claimed_until
    }

    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    #[must_use]
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    #[must_use]
    pub fn idempotency_key(&self) -> Option<&str> {
        self.idempotency_key.as_deref()
    }

    #[must_use]
    pub fn idempotency_expires_at(&self) -> Option<DateTime<Utc>> {
        self.idempotency_expires_at
    }

    #[must_use]
    pub fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    #[must_use]
    pub fn checkpoint(&self) -> Option<&serde_json::Value> {
        self.checkpoint.as_deref()
    }

    #[must_use]
    pub fn suspended_at(&self) -> Option<DateTime<Utc>> {
        self.suspended_at
    }

    #[must_use]
    pub fn signal_payload(&self) -> Option<&serde_json::Value> {
        self.signal_payload.as_deref()
    }

    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    pub fn validate_invariants(&self) {
        debug_assert!(
            self.created_at <= self.updated_at,
            "created_at must not be after updated_at"
        );
        debug_assert!(
            self.claimed_by.is_some() == self.claimed_until.is_some(),
            "claimed_by and claimed_until must be set/unset together"
        );
    }

    #[must_use]
    pub fn payload_arc(&self) -> &Arc<serde_json::Value> {
        &self.payload
    }

    #[must_use]
    pub fn into_payload(self) -> Arc<serde_json::Value> {
        self.payload
    }

    pub fn take_payload(&mut self) -> Arc<serde_json::Value> {
        std::mem::replace(&mut self.payload, Arc::new(serde_json::Value::Null))
    }

    pub fn take_checkpoint(&mut self) -> Option<Arc<serde_json::Value>> {
        self.checkpoint.take()
    }

    pub fn take_signal_payload(&mut self) -> Option<Arc<serde_json::Value>> {
        self.signal_payload.take()
    }

    #[must_use]
    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = status;
        self
    }

    #[must_use]
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = Arc::new(payload);
        self
    }
}

/// Outcome of a task cancellation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum CancelResult {
    /// The task was successfully cancelled. Contains the updated record.
    Cancelled(TaskRecord),
    /// No task exists with the given ID.
    NotFound,
    /// The task exists but is not in a cancellable state.
    NotCancellable { current_status: TaskStatus },
}

/// Filter criteria for listing tasks with pagination.
#[derive(Debug, Clone)]
pub struct ListTasksFilter {
    pub queue: Option<QueueName>,
    pub status: Option<TaskStatus>,
    pub limit: u32,
    pub offset: u32,
}

/// Paginated result from a filtered task list query.
#[derive(Debug, Clone)]
pub struct ListTasksResult {
    pub tasks: Vec<TaskRecord>,
    pub total: u64,
}

/// Runtime context handed to a `Task::execute` invocation.
///
/// Marked `#[non_exhaustive]` to allow forward-compatible field additions
/// without breaking downstream `Task` implementations.
///
/// `attempt` is `i32` to match `TaskRecord::attempts` and avoid a fallible
/// `i32 → u32` conversion at the execution boundary. The Postgres `INTEGER`
/// type is signed; downstream code can rely on `attempt >= 1` once the
/// claiming protocol increments it.
///
/// The context carries `task_id`, `worker_id`, and `attempt` so handlers have
/// stable identity and retry metadata for checkpointing and control-flow APIs.
/// Port for persisting checkpoint data during task execution.
///
/// Defined in the domain crate to keep `TaskContext` free of infrastructure
/// dependencies (`sqlx`, `PgPool`). The infrastructure layer provides the
/// concrete implementation.
pub trait CheckpointWriter: Send + Sync {
    fn write_checkpoint(
        &self,
        task_id: TaskId,
        worker_id: WorkerId,
        data: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + '_>>;
}

const CHECKPOINT_MAX_BYTES: usize = 1_048_576;
pub const SIGNAL_PAYLOAD_MAX_BYTES: usize = 1_048_576; // 1 MiB

#[derive(Clone)]
#[non_exhaustive]
pub struct TaskContext {
    pub(crate) task_id: TaskId,
    pub(crate) worker_id: WorkerId,
    pub(crate) attempt: AttemptCount,
    pub(crate) last_checkpoint: Option<serde_json::Value>,
    pub(crate) checkpoint_writer: Option<Arc<dyn CheckpointWriter>>,
    pub(crate) signal_payload: Option<serde_json::Value>,
}

impl std::fmt::Debug for TaskContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskContext")
            .field("task_id", &self.task_id)
            .field("worker_id", &self.worker_id)
            .field("attempt", &self.attempt)
            .field("last_checkpoint", &self.last_checkpoint.is_some())
            .field("checkpoint_writer", &self.checkpoint_writer.is_some())
            .field("signal_payload", &self.signal_payload.is_some())
            .finish()
    }
}

impl TaskContext {
    #[must_use]
    pub fn new(task_id: TaskId, worker_id: WorkerId, attempt: AttemptCount) -> Self {
        Self {
            task_id,
            worker_id,
            attempt,
            last_checkpoint: None,
            checkpoint_writer: None,
            signal_payload: None,
        }
    }

    #[must_use]
    pub fn with_checkpoint(
        mut self,
        last_checkpoint: Option<serde_json::Value>,
        writer: Arc<dyn CheckpointWriter>,
    ) -> Self {
        self.last_checkpoint = last_checkpoint;
        self.checkpoint_writer = Some(writer);
        self
    }

    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    #[must_use]
    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    #[must_use]
    pub fn attempt(&self) -> AttemptCount {
        self.attempt
    }

    #[must_use]
    pub fn with_signal_payload(mut self, payload: Option<serde_json::Value>) -> Self {
        self.signal_payload = payload;
        self
    }

    #[must_use]
    pub fn last_checkpoint(&self) -> Option<&serde_json::Value> {
        self.last_checkpoint.as_ref()
    }

    #[must_use]
    pub fn signal_payload(&self) -> Option<&serde_json::Value> {
        self.signal_payload.as_ref()
    }

    pub async fn suspend(
        &self,
        checkpoint_data: Option<serde_json::Value>,
    ) -> Result<(), TaskError> {
        if let Some(data) = checkpoint_data {
            self.checkpoint(data).await?;
        }
        Err(TaskError::SuspendRequested)
    }

    pub async fn checkpoint(&self, data: serde_json::Value) -> Result<(), TaskError> {
        // Serialize once to check size and handle serialization failures explicitly
        // instead of defaulting to a size error.
        let serialized = serde_json::to_vec(&data).map_err(|e| TaskError::InvalidPayload {
            kind: crate::error::PayloadErrorKind::Serialization {
                message: format!("failed to serialize checkpoint data: {e}"),
            },
        })?;

        let serialized_len = serialized.len();
        if serialized_len > CHECKPOINT_MAX_BYTES {
            return Err(TaskError::InvalidPayload {
                kind: crate::error::PayloadErrorKind::Validation {
                    message: format!(
                        "checkpoint data size ({serialized_len} bytes) exceeds 1 MiB limit"
                    ),
                },
            });
        }

        let writer = self
            .checkpoint_writer
            .as_ref()
            .ok_or_else(|| TaskError::ExecutionFailed {
                kind: crate::error::ExecutionErrorKind::HandlerFailed {
                    source: "checkpoint writer not available".into(),
                },
            })?;

        writer
            .write_checkpoint(self.task_id, self.worker_id, data)
            .await
    }
}

/// User-facing task contract.
///
/// Implementors define a stable `KIND` discriminator and the asynchronous
/// execution body. Uses native `async fn` in trait — no `async-trait` proc
/// macro is needed at the user-facing layer. (Native `async fn` in trait
/// stabilized in Rust 1.75; iron-defer's MSRV of 1.94 is driven by other
/// workspace dependencies, not this trait.) Type erasure for the registry
/// happens behind a separate object-safe `TaskHandler` trait in the
/// application layer.
///
/// **Serde supertrait bounds (architecture §D7.2):** `Serialize +
/// DeserializeOwned` are required because the registry's
/// `TaskHandlerAdapter<T>` round-trips `T → serde_json::Value → T` for
/// type-erased dispatch. Without these bounds the engine could not store
/// `T` payloads in the database and reconstruct them at execute time.
pub trait Task: Send + Sync + Serialize + DeserializeOwned + 'static {
    /// Stable string discriminator for this task type. Persisted in the
    /// `tasks.kind` column and used for handler dispatch.
    const KIND: &'static str;

    /// Execute the task. Implementations should be idempotent because the
    /// engine guarantees at-least-once execution.
    fn execute(&self, ctx: &TaskContext) -> impl Future<Output = Result<(), TaskError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Cancelled).unwrap(),
            "\"cancelled\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Suspended).unwrap(),
            "\"suspended\""
        );
    }

    #[test]
    fn task_status_round_trips() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Suspended,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn task_id_round_trips() {
        let id = TaskId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    /// Smoke test: confirm the `Task` trait can be implemented with native
    /// `async fn` and serde supertrait bounds without `async-trait` plumbing.
    #[derive(Serialize, Deserialize)]
    struct DummyTask;

    impl Task for DummyTask {
        const KIND: &'static str = "dummy";

        async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
            Ok(())
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn validate_invariants_panics_on_created_after_updated() {
        let now = chrono::Utc::now();
        let earlier = now - chrono::Duration::seconds(10);
        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(QueueName::try_from("test").unwrap())
            .kind(TaskKind::try_from("test").unwrap())
            .payload(Arc::new(serde_json::json!({})))
            .status(TaskStatus::Pending)
            .priority(crate::model::priority::Priority::new(0))
            .attempts(crate::model::attempts::AttemptCount::ZERO)
            .max_attempts(crate::model::attempts::MaxAttempts::DEFAULT)
            .scheduled_at(now)
            .created_at(now)
            .updated_at(earlier)
            .build();
        let result = std::panic::catch_unwind(|| record.validate_invariants());
        assert!(
            result.is_err(),
            "expected debug_assert to fire for created_at > updated_at"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn validate_invariants_panics_on_claimed_by_without_claimed_until() {
        let now = chrono::Utc::now();
        let record = TaskRecord::builder()
            .id(TaskId::new())
            .queue(QueueName::try_from("test").unwrap())
            .kind(TaskKind::try_from("test").unwrap())
            .payload(Arc::new(serde_json::json!({})))
            .status(TaskStatus::Running)
            .priority(crate::model::priority::Priority::new(0))
            .attempts(crate::model::attempts::AttemptCount::new(1).unwrap())
            .max_attempts(crate::model::attempts::MaxAttempts::DEFAULT)
            .scheduled_at(now)
            .claimed_by(crate::model::worker::WorkerId::new())
            .created_at(now)
            .updated_at(now)
            .build();
        let result = std::panic::catch_unwind(|| record.validate_invariants());
        assert!(
            result.is_err(),
            "expected debug_assert to fire for claimed_by without claimed_until"
        );
    }

    /// P0-UNIT-001 — all five `TaskStatus` variants exist and cover the expected
    /// lifecycle states. This is a domain-level completeness guard: if a new
    /// variant is added or one is removed, this test must be updated.
    #[test]
    fn task_status_covers_expected_lifecycle_states() {
        let all_statuses = [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Suspended,
        ];
        assert_eq!(all_statuses.len(), 6, "expected exactly 6 lifecycle states");

        let names: Vec<String> = all_statuses
            .iter()
            .map(|s| serde_json::to_string(s).unwrap())
            .collect();
        assert!(names.contains(&"\"pending\"".to_string()));
        assert!(names.contains(&"\"running\"".to_string()));
        assert!(names.contains(&"\"completed\"".to_string()));
        assert!(names.contains(&"\"failed\"".to_string()));
        assert!(names.contains(&"\"cancelled\"".to_string()));
        assert!(names.contains(&"\"suspended\"".to_string()));
    }

    /// P0-UNIT-002 — invalid/unknown status strings are rejected by the
    /// deserializer. The engine must never silently accept an unknown
    /// state value from external input.
    #[test]
    fn task_status_rejects_unknown_variant() {
        let invalid = [
            "\"active\"",
            "\"queued\"",
            "\"processing\"",
            "\"done\"",
            "\"\"",
            "\"PENDING\"",
        ];
        for s in invalid {
            let result = serde_json::from_str::<TaskStatus>(s);
            assert!(
                result.is_err(),
                "expected rejection for {s}, got {:?}",
                result.unwrap()
            );
        }
    }

    #[test]
    fn task_trait_serde_round_trips() {
        // Smoke test for the Serialize + DeserializeOwned supertrait bounds.
        // Confirms round-tripping a `T: Task` through serde_json (which is exactly
        // what TaskHandlerAdapter relies on for type-erased dispatch).
        let task = DummyTask;
        let value = serde_json::to_value(&task).expect("serialize");
        let _round_tripped: DummyTask = serde_json::from_value(value).expect("deserialize");
    }

    #[test]
    fn task_record_arc_payload_serde_round_trip() {
        let now = chrono::Utc::now();
        let original = TaskRecord::builder()
            .id(TaskId::new())
            .queue(QueueName::try_from("test").unwrap())
            .kind(TaskKind::try_from("test").unwrap())
            .payload(Arc::new(serde_json::json!({"key": "value", "n": 42})))
            .status(TaskStatus::Pending)
            .priority(crate::model::priority::Priority::new(0))
            .attempts(crate::model::attempts::AttemptCount::ZERO)
            .max_attempts(crate::model::attempts::MaxAttempts::DEFAULT)
            .scheduled_at(now)
            .created_at(now)
            .updated_at(now)
            .build();

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: TaskRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn arc_value_equality_compares_inner_values() {
        let a = Arc::new(serde_json::json!({"a": 1}));
        let b = Arc::new(serde_json::json!({"a": 1}));
        assert_eq!(a, b, "Arc<Value> equality should compare inner values");
    }
}
