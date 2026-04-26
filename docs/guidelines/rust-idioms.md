# Rust Idioms and Best Practices

This guide documents the idiomatic Rust patterns required in iron-defer. These are not suggestions — they are the baseline for code review approval.

---

## Newtype Pattern

### Purpose

Prevent primitive obsession. Encode domain meaning into the type system. Make invalid states unrepresentable.

### Rules

- Every domain identifier gets a newtype: `UserId(Uuid)`, `TaskId(Uuid)`, `WorkerId(Uuid)`
- Every domain value with constraints gets a newtype: `EmailAddress(String)`, `TaskName(String)`
- Inner fields are **never `pub`** — enforce invariants through methods
- Use `#[serde(transparent)]` for transparent JSON representation

### Template

```rust
use std::fmt;
use uuid::Uuid;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);

impl TaskId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for TaskId {
    type Error = uuid::Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for TaskId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}
```

### Value-Constrained Newtypes

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskName(String);

impl TaskName {
    pub fn new(value: impl Into<String>) -> Result<Self, TaskError> {
        let s = value.into();
        if s.is_empty() {
            return Err(TaskError::InvalidName { reason: "name must not be empty".into() });
        }
        if s.len() > 255 {
            return Err(TaskError::InvalidName { reason: "name too long (max 255 chars)".into() });
        }
        Ok(Self(s))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

---

## Trait Design

### Interface Segregation

Keep traits small and focused. One behavior per trait.

```rust
// Good: granular traits
#[async_trait]
pub trait TaskReader: Send + Sync {
    async fn find_by_id(&self, id: &TaskId) -> Result<Task, TaskError>;
    async fn list_pending(&self, limit: u32) -> Result<Vec<Task>, TaskError>;
}

#[async_trait]
pub trait TaskWriter: Send + Sync {
    async fn save(&self, task: &Task) -> Result<(), TaskError>;
    async fn delete(&self, id: &TaskId) -> Result<(), TaskError>;
}

// A full repository combines both
pub trait TaskRepository: TaskReader + TaskWriter {}
```

### Marker Traits and Blanket Implementations

```rust
// Blanket impl: anything that is TaskReader + TaskWriter is automatically TaskRepository
impl<T: TaskReader + TaskWriter> TaskRepository for T {}
```

### Extension Traits

Add behavior to external types or your own types in context-specific ways:

```rust
pub trait OptionTaskExt<T> {
    fn or_not_found(self, id: &TaskId) -> Result<T, TaskError>;
}

impl<T> OptionTaskExt<T> for Option<T> {
    fn or_not_found(self, id: &TaskId) -> Result<T, TaskError> {
        self.ok_or_else(|| TaskError::NotFound { id: id.clone() })
    }
}

// Usage:
let task = repo.find_optional(id).await?.or_not_found(id)?;
```

### Object Safety

Traits used as `Arc<dyn Trait>` must be object-safe:
- No generic methods (use associated types instead if needed)
- No `Self` in return types (unless `where Self: Sized`)
- No static methods

---

## Generics vs Trait Objects

| Use Case | Prefer |
|----------|--------|
| Function argument — zero-cost, monomorphized | `impl Trait` |
| Stored in struct, runtime polymorphism needed | `Arc<dyn Trait>` |
| Multiple implementations chosen at runtime | `Arc<dyn Trait>` |
| Performance-critical hot path | generics `<T: Trait>` |
| Simple conversion or adapter | `impl Into<T>` / `impl AsRef<T>` |

```rust
// Function arg: impl Trait (zero-cost)
pub fn process(handler: impl TaskHandler) { ... }

// Stored state: Arc<dyn Trait> (runtime polymorphism)
pub struct Scheduler {
    repository: Arc<dyn TaskRepository>,
    executor: Arc<dyn TaskExecutor>,
}

// Flexible constructors: impl Into<T>
pub fn with_name(mut self, name: impl Into<TaskName>) -> Result<Self, TaskError> {
    self.name = name.into()?;
    Ok(self)
}
```

---

## Builder Pattern

Use the builder pattern for structs with more than 2-3 optional fields:

```rust
#[derive(Debug)]
pub struct Task {
    id: TaskId,
    name: TaskName,
    payload: serde_json::Value,
    scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    max_retries: u32,
}

#[derive(Debug, Default)]
pub struct TaskBuilder {
    name: Option<TaskName>,
    payload: Option<serde_json::Value>,
    scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    max_retries: u32,
}

impl TaskBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self { max_retries: 3, ..Default::default() }
    }

    pub fn name(mut self, name: impl TryInto<TaskName, Error = TaskError>) -> Result<Self, TaskError> {
        self.name = Some(name.try_into()?);
        Ok(self)
    }

    #[must_use]
    pub fn payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = Some(payload);
        self
    }

    #[must_use]
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    pub fn build(self) -> Result<Task, TaskError> {
        Ok(Task {
            id: TaskId::new(),
            name: self.name.ok_or(TaskError::InvalidPayload {
                reason: "name is required".into(),
            })?,
            payload: self.payload.unwrap_or(serde_json::Value::Null),
            scheduled_at: self.scheduled_at,
            max_retries: self.max_retries,
        })
    }
}
```

---

## `#[must_use]`

Apply `#[must_use]` to:
- Constructor methods
- Any function returning `Result` or `Option`
- Builder method chains
- Any function with a meaningful return value that is easy to accidentally discard

```rust
impl TaskId {
    #[must_use]
    pub fn new() -> Self { ... }
}

#[must_use]
pub fn validate_payload(value: &serde_json::Value) -> Result<(), TaskError> { ... }
```

---

## Ownership and Borrowing Conventions

| Scenario | Use |
|----------|-----|
| Read-only string parameter | `&str` not `&String` |
| Accept both `String` and `&str` | `impl AsRef<str>` or `impl Into<String>` |
| Read-only slice | `&[T]` not `&Vec<T>` |
| Returning owned string | `String` |
| Cheap clone for sharing | `Arc<T>` not `.clone()` everywhere |

```rust
// Prefer:
pub fn find_by_name(&self, name: &str) -> Result<Task, TaskError> { ... }
pub fn with_label(mut self, label: impl Into<String>) -> Self { ... }

// Not:
pub fn find_by_name(&self, name: &String) -> Result<Task, TaskError> { ... }
```

---

## Enum Best Practices

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]  // allow adding variants without breaking downstream
pub enum TaskStatus {
    Pending,
    Running { worker_id: WorkerId },
    Completed { at: chrono::DateTime<chrono::Utc> },
    Failed { attempts: u32, last_error: String },
    Cancelled,
}

impl TaskStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled)
    }

    #[must_use]
    pub fn can_retry(&self) -> bool {
        matches!(self, Self::Failed { attempts, .. } if *attempts < MAX_RETRIES)
    }
}
```

Use `#[non_exhaustive]` on public enums to allow adding variants without a semver break. Match patterns must use `_` to handle future variants.

---

## String Formatting and Display

```rust
use std::fmt;

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running { worker_id } => write!(f, "running (worker: {worker_id})"),
            Self::Completed { at } => write!(f, "completed at {at}"),
            Self::Failed { attempts, .. } => write!(f, "failed after {attempts} attempts"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}
```

Prefer `write!(f, "{var}")` (Rust 1.58+ captured identifiers) over `write!(f, "{}", var)`.

---

## Payload-Privacy Discipline (FR38)

Task payloads are opaque, caller-supplied JSON and may contain PII or secrets. Every `#[instrument]` or `tracing::event!` site that touches task state MUST prevent the payload from being serialized into a log record by default.

### Rules

1. **`#[instrument(skip(...))]`** — any async method whose arguments include a `TaskRecord`, `payload`, `task`, or `error_message` MUST `skip(...)` those arguments so they never appear in the span's automatic field capture. See `crates/infrastructure/src/adapters/postgres_task_repository.rs:175, 348` and `crates/application/src/services/scheduler.rs:63, 117` for the canonical pattern.

2. **Conditional `payload = ?…` fields** — lifecycle log events (`task_enqueued`, `task_claimed`, `task_completed`, `task_failed_retry`, `task_failed_terminal`) may include the payload only when the caller has explicitly opted in via `WorkerConfig::log_payload = true` (FR39). The safe idiom is two branches per emission site:

   ```rust
   if log_payload {
       info!(event = "task_completed", task_id = %task.id, payload = ?task.payload, ...);
   } else {
       info!(event = "task_completed", task_id = %task.id, ...);
   }
   ```

   Do NOT use `payload = Option<..>` with `None` as the "hidden" value — `tracing` will serialize `None` literally and the field name alone leaks that a payload exists.

3. **Never log the DB URL** — `sqlx::Error::Configuration` source text may carry the connection string. Use `iron_defer_infrastructure::scrub_url` at the adapter boundary when forwarding such errors (NFR-S2).

4. **No payload in `fields(...)` either** — `#[instrument(fields(payload = %task.payload))]` is equivalent to a leak, regardless of the `skip(...)` list. `fields(...)` values are always serialized.

5. **Never put payload content in error messages** — `TaskError::ExecutionFailed { reason }` (and any other stringly-typed error variant) is forwarded into the `error` structured field of `task_failed_retry`, `task_failed_terminal`, and `task_fail_storage_error` **regardless of `log_payload`**. A handler that writes `"failed to process record: {payload}"` into `reason` leaks the payload through the error channel, defeating FR38's default-off guarantee. Keep error messages structural (operation name, failure class, DB error code) — payload context belongs in the `payload = ?task.payload` field, gated on `log_payload`, not in stringified errors.

   ```rust
   // ❌ WRONG — payload leaks via error field even when log_payload=false
   return Err(TaskError::ExecutionFailed {
       reason: format!("could not deserialize {:?}", payload),
   });

   // ✅ RIGHT — structural; payload content stays out of the error channel
   return Err(TaskError::ExecutionFailed {
       reason: "payload deserialization failed".to_string(),
   });
   ```

### When adding a new instrument site

Before merging any PR that adds `#[instrument]` or `tracing::event!` to code handling task records, walk the audit:

- [ ] All `TaskRecord` / `payload` arguments are in `skip(...)`.
- [ ] No `fields(...)` entry serializes payload content.
- [ ] If the new site is a lifecycle event, payload is gated on `log_payload`.
- [ ] A `payload_privacy_*` unit test covers the new event (see `crates/application/src/services/worker.rs::tests`).
- [ ] Error messages produced by the handler (or the adapter) contain no payload content — structural diagnostics only (Rule 5).

---

## Clippy Pedantic Compliance

Crate root attribute (every crate):

```rust
#![deny(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]  // document allowed exceptions here
```

Common fixes required by pedantic:
- `into_iter()` → iterator adaptors
- `map_or` instead of `map().unwrap_or()`
- `must_use` on pure functions
- Explicit `as` cast documentation or replacement with `From`/`Into`
- `match` over `if let` for complex patterns

---

## References

- [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/)
- [The Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- [Rust Design Patterns](https://rust-unofficial.github.io/patterns/)
- [Clippy Lints](https://rust-lang.github.io/rust-clippy/master/)
