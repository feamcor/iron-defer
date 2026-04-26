# Story 6.10: Field Visibility & Accessor Methods

Status: done

## Story

As a developer,
I want domain struct fields to be private with typed accessors,
so that invariants are enforced and future field type changes (e.g., `payload` to `Arc<serde_json::Value>` in Epic 7) don't break callers.

## Acceptance Criteria

1. **Private fields with accessors**

   **Given** domain model structs (`TaskRecord`, `QueueName`, `TaskId`, `WorkerId`)
   **When** I inspect their field visibility
   **Then** inner fields are `pub(crate)` or private — not `pub`
   **And** accessor methods are provided for each field (e.g., `pub fn status(&self) -> TaskStatus` for Copy types, `pub fn payload(&self) -> &serde_json::Value` for non-Copy types)
   **And** construction is only possible through validated constructors or builders (from Story 6.9)

2. **`From<TaskRecord> for TaskResponse` migration**

   **Given** the `From<TaskRecord> for TaskResponse` impl in `tasks.rs`
   **When** it accesses TaskRecord fields
   **Then** it uses the new accessor methods instead of direct field access
   **And** all other direct field accesses across the codebase (30+ sites in `worker.rs`, handlers, tests) are migrated to accessors

3. **Public API surface unchanged**

   **Given** all field visibility changes
   **When** `cargo test --workspace` runs
   **Then** all tests pass
   **And** the public API surface (`IronDefer`, `Task` trait, `TaskContext`, `TaskError`) is unchanged or only gains accessor methods

## Tasks / Subtasks

- [x] **Task 1: Add accessor methods to `TaskRecord`** (AC: 1)
  - [x] 1.1: In `crates/domain/src/model/task.rs`, add accessor methods for all 14 fields:
    - `pub fn id(&self) -> TaskId` (Copy)
    - `pub fn queue(&self) -> &QueueName`
    - `pub fn kind(&self) -> &TaskKind`
    - `pub fn payload(&self) -> &serde_json::Value`
    - `pub fn status(&self) -> TaskStatus` (Copy)
    - `pub fn priority(&self) -> Priority` (Copy)
    - `pub fn attempts(&self) -> AttemptCount` (Copy)
    - `pub fn max_attempts(&self) -> MaxAttempts` (Copy)
    - `pub fn last_error(&self) -> Option<&str>`
    - `pub fn scheduled_at(&self) -> DateTime<Utc>` (Copy)
    - `pub fn claimed_by(&self) -> Option<WorkerId>` (Copy)
    - `pub fn claimed_until(&self) -> Option<DateTime<Utc>>` (Copy)
    - `pub fn created_at(&self) -> DateTime<Utc>` (Copy)
    - `pub fn updated_at(&self) -> DateTime<Utc>` (Copy)
  - [x] 1.2: Add `pub fn into_payload(self) -> serde_json::Value` for move semantics where needed

- [x] **Task 2: Change `TaskRecord` fields to `pub(crate)`** (AC: 1)
  - [x] 2.1: Change all 14 `pub` fields to `pub(crate)` in `task.rs:78–91`
  - [x] 2.2: Verify domain-crate internal code still compiles (constructor, builder, serde)
  - [x] 2.3: Fix all compilation errors in external crates (application, infrastructure, api) by switching to accessor methods

- [x] **Task 3: Migrate application crate field accesses** (AC: 2)
  - [x] 3.1: `crates/application/src/services/worker.rs` — ~30 direct field accesses in dispatch_task, emit functions, and tracing spans
  - [x] 3.2: `crates/application/src/services/scheduler.rs` — field accesses in enqueue methods and test assertions

- [x] **Task 4: Migrate infrastructure crate field accesses** (AC: 2)
  - [x] 4.1: `crates/infrastructure/src/adapters/postgres_task_repository.rs` — 14 fields accessed in `save()` INSERT query (lines 215–226), plus tracing spans
  - [x] 4.2: `crates/infrastructure/tests/task_repository_test.rs` — 25+ field assertions in tests

- [x] **Task 5: Migrate api crate field accesses** (AC: 2)
  - [x] 5.1: `crates/api/src/http/handlers/tasks.rs` — `From<TaskRecord> for TaskResponse` impl (lines 70–89)
  - [x] 5.2: `crates/api/src/cli/output.rs` — CLI formatting (lines 35–81)
  - [x] 5.3: `crates/api/src/lib.rs` — tracing spans and emit functions
  - [x] 5.4: `crates/api/examples/` — `basic_enqueue.rs` and `axum_integration.rs`
  - [x] 5.5: `crates/api/tests/` — 8+ test files with field assertions

- [x] **Task 6: Add accessor methods to `TaskContext`** (AC: 1)
  - [x] 6.1: Add accessors: `pub fn task_id(&self) -> TaskId`, `pub fn worker_id(&self) -> WorkerId`, `pub fn attempt(&self) -> AttemptCount`
  - [x] 6.2: Change fields to `pub(crate)`
  - [x] 6.3: Migrate all `TaskContext` field accesses to use accessors

- [x] **Task 7: Verify no regressions** (AC: 3)
  - [x] 7.1: `cargo test --workspace` — all tests pass
  - [x] 7.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings
  - [x] 7.3: `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **Public API surface (architecture lines 930–939):** `TaskRecord` is re-exported in the public API (`crates/api/src/lib.rs:83`). Adding accessors is a backward-compatible API extension. Changing fields from `pub` to `pub(crate)` is a breaking change for any external consumer who accesses fields directly — but since the struct is `#[non_exhaustive]` and this is pre-1.0, it's acceptable.
- **Crate boundaries (architecture lines 924–937):** `pub(crate)` restricts field access to the defining crate (`domain`). All other crates (`application`, `infrastructure`, `api`) must use the public accessors.
- **NFR-M3 (semver/public API):** This is a technically breaking change (fields go from `pub` to `pub(crate)`). Acceptable pre-1.0 and explicitly part of the CR46 hardening pass.

### Critical Implementation Guidance

**Accessor return type strategy:**

For `Copy` types (TaskId, TaskStatus, Priority, AttemptCount, MaxAttempts, DateTime<Utc>, WorkerId), return by value:
```rust
pub fn id(&self) -> TaskId { self.id }
pub fn status(&self) -> TaskStatus { self.status }
pub fn scheduled_at(&self) -> DateTime<Utc> { self.scheduled_at }
```

For non-Copy types (QueueName, TaskKind, serde_json::Value, String), return by reference:
```rust
pub fn queue(&self) -> &QueueName { &self.queue }
pub fn kind(&self) -> &TaskKind { &self.kind }
pub fn payload(&self) -> &serde_json::Value { &self.payload }
pub fn last_error(&self) -> Option<&str> { self.last_error.as_deref() }
```

For `Option<WorkerId>` (Copy inner type):
```rust
pub fn claimed_by(&self) -> Option<WorkerId> { self.claimed_by }
```

**Cross-epic dependency (Epic 7, Story 7.2):**

Story 7.2 changes `TaskRecord.payload` from `serde_json::Value` to `Arc<serde_json::Value>`. The accessor `pub fn payload(&self) -> &serde_json::Value` will continue to work via `Arc::Deref` coercion — the signature doesn't need to change. This is the primary motivation for encapsulation.

**`into_payload` for move semantics:**

Some code paths need to take ownership of the payload (e.g., `dispatch_task` clones it for the handler). Add a consuming accessor:
```rust
pub fn into_payload(self) -> serde_json::Value { self.payload }
```

When Story 7.2 changes the inner type to `Arc<serde_json::Value>`, this becomes `pub fn into_payload(self) -> Arc<serde_json::Value>` — callers that need ownership already use this method.

**`save()` method in postgres_task_repository.rs — the biggest migration:**

The `save()` method (lines 193–234) passes ALL 14 fields to the INSERT query:
```rust
// Current (direct field access):
task.id.as_uuid(),
task.queue.as_str(),
task.kind.as_str(),
task.payload,           // ← this is a move, not borrow
task.status,
task.priority.get(),
// ...
```

With accessors:
```rust
// Migrated (accessor methods):
task.id().as_uuid(),
task.queue().as_str(),
task.kind().as_str(),
&task.payload(),        // ← borrow from accessor reference
task.status(),
task.priority().get(),
// ...
```

**Important:** The `save()` method takes `&TaskRecord`, not owned `TaskRecord`. Field accesses via accessors that return references (like `payload()`) return `&&serde_json::Value` — sqlx `Encode` trait handles this via auto-deref. Verify during implementation.

**`serde` + `pub(crate)` interaction:** `TaskRecord` derives `Deserialize` (line 75). Serde-generated deserialization code operates at the crate level where `pub(crate)` fields are accessible, so deserialization is unaffected. The `Deserialize` derive is present for completeness but is NOT used in production code — `TaskRecord` is always constructed programmatically via `new()`, builder, or `TryFrom<TaskRow>`. No serde-related breakage expected.

**`save()` double-reference pattern:** When `save()` calls `task.payload()` (which returns `&serde_json::Value`) and passes it to `sqlx::query_as!(.., task.payload(), ..)`, the binding is `&&serde_json::Value`. sqlx's `Encode` trait auto-derefs through references. If this causes compilation issues, the fix is to dereference explicitly: `let payload = task.payload(); ... .bind(payload) ...` — binding a local `&serde_json::Value` rather than a double-reference.

**Worker emit functions — the largest volume of accesses:**

The emit functions in `worker.rs` (lines 587–878) access task fields for tracing spans. These are all `&TaskRecord` contexts:

```rust
// Current:
info!(task_id = %task.id, queue = %task.queue, kind = %task.kind, ...);

// Migrated:
info!(task_id = %task.id(), queue = %task.queue(), kind = %task.kind(), ...);
```

The `%` format specifier calls `Display::fmt()` on the result. Since `task.id()` returns `TaskId` (which implements `Display`), and `task.queue()` returns `&QueueName` (which implements `Display`), the tracing macros work identically.

**Test assertion migration:**

Test code that asserts on fields:
```rust
// Current:
assert_eq!(found.status, TaskStatus::Completed);
assert_eq!(found.attempts.get(), 2);

// Migrated:
assert_eq!(found.status(), TaskStatus::Completed);
assert_eq!(found.attempts().get(), 2);
```

For tests in the `domain` crate itself (inline `#[cfg(test)]` modules), direct field access still works since `pub(crate)` grants access within the crate. Only tests in other crates need migration.

**Test mutation sites:** Two tests in `worker.rs` MUTATE `TaskRecord` fields directly:
- Line 1352: `task.payload = payload;`
- Line 1641: `task.payload = serde_json::json!({});`

These won't compile with `pub(crate)` fields. Fix by constructing the `TaskRecord` with the desired payload from the start (via builder from Story 6.9) instead of mutating post-construction.

**Migration order recommendation:**

1. First: Add all accessor methods (Task 1) — this is backward-compatible
2. Second: Migrate ALL external field accesses to use accessors (Tasks 3–5) — still backward-compatible, since `pub` fields coexist with accessors
3. Third: Change fields to `pub(crate)` (Task 2) — now only domain-internal code can access fields directly
4. Fourth: Fix any remaining compilation errors

This order ensures the codebase compiles at every intermediate step.

**`TaskContext` is simpler:**

Only 3 fields, used in:
- `dispatch_task` in worker.rs (constructed via `TaskContext::new()`)
- User `Task::execute()` implementations (accessed by users)
- Test task impls

Since `TaskContext` is part of the public API (users receive it in `Task::execute`), the accessors become the user-facing contract. The fields are simple Copy types:
```rust
pub fn task_id(&self) -> TaskId { self.task_id }
pub fn worker_id(&self) -> WorkerId { self.worker_id }
pub fn attempt(&self) -> AttemptCount { self.attempt }
```

### Encapsulation status of other domain types (already done — NO changes needed)

| Type | Inner | Private? | Accessors | Action |
|------|-------|----------|-----------|--------|
| `TaskId` | `Uuid` | YES | `as_uuid()` | None |
| `QueueName` | `String` | YES | `as_str()`, `into_inner()` | None |
| `WorkerId` | `Uuid` | YES | `as_uuid()` | None |
| `Priority` | `i16` | YES | `get()` | None |
| `AttemptCount` | `i32` | YES | `get()` | None |
| `MaxAttempts` | `i32` | YES | `get()` | None |
| `TaskKind` | `String` | YES | `as_str()`, `into_inner()` | None |

### Previous Story Intelligence

**From Story 6.9 (ready-for-dev):**
- `TaskRecord` gains `#[derive(bon::Builder)]`. The builder generates construction methods that set private fields — this is how external crates will construct `TaskRecord` after fields become `pub(crate)`. The builder and accessors together form the complete public interface.
- `WorkerService` gains builder pattern. `WorkerService` is NOT in the public API — no accessor changes needed there.
- `DispatchContext` struct created. Its fields access `TaskRecord` — those accesses must use the new accessors.

**From Story 6.3 (ready-for-dev):**
- `TaskStatus` has `#[non_exhaustive]`. Accessor `pub fn status(&self) -> TaskStatus` returns the Copy enum value — works correctly with `#[non_exhaustive]`.

### Git Intelligence

Last code commit: `7ed6fc8`. `TaskRecord` fields last modified in Story 1A.1 (initial domain model).

### Key Types and Locations (verified current)

| Type | Location | Action |
|---|---|---|
| `TaskRecord` struct | `crates/domain/src/model/task.rs:77–92` | Add accessors, change to `pub(crate)` |
| `TaskRecord::new()` | `task.rs:108–141` | Kept (or removed if builder from 6.9 replaces it) |
| `TaskContext` struct | `task.rs:191–208` | Add accessors, change to `pub(crate)` |
| `From<TaskRecord> for TaskResponse` | `crates/api/src/http/handlers/tasks.rs:70–89` | Migrate to accessors |
| CLI output formatting | `crates/api/src/cli/output.rs:35–81` | Migrate to accessors |
| `save()` INSERT query | `postgres_task_repository.rs:193–234` | Migrate field bindings to accessors |
| Worker emit functions | `worker.rs:587–878` | Migrate tracing spans to accessors |
| `dispatch_task` | `worker.rs:382–556` | Migrate field accesses to accessors |
| Scheduler enqueue | `scheduler.rs:84, 149` | Builder handles construction (from 6.9) |
| Public API re-exports | `crates/api/src/lib.rs:70–86` | `TaskRecord` re-exported — gains accessor methods |

### Scale of Migration

**Estimated ~470+ direct field accesses across 16 files:**
- `worker.rs` — ~30 accesses (tracing spans, emit functions, dispatch)
- `postgres_task_repository.rs` — ~20 accesses (save INSERT, tracing spans)
- `task_repository_test.rs` — ~25 accesses (assertions)
- `output.rs` — ~10 accesses (CLI formatting)
- `tasks.rs` (handlers) — ~14 accesses (TaskResponse conversion)
- `lib.rs` (api) — ~15 accesses (tracing, emit functions)
- `scheduler.rs` — ~10 accesses (construction, test assertions)
- Test files (8 files) — ~30 accesses (assertions on `record.id`, `record.status`, etc.)
- Examples (2 files) — ~8 accesses

The migration is mechanical but high-volume. Each `record.field` becomes `record.field()` (add parentheses). For tracing `%` format fields, the change is transparent since Display is called on the result.

### Dependencies

No new crate dependencies.

### Project Structure Notes

- **Modified files:**
  - `crates/domain/src/model/task.rs` — add accessors, change visibility
  - `crates/application/src/services/worker.rs` — migrate ~30 field accesses
  - `crates/application/src/services/scheduler.rs` — migrate ~10 field accesses
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — migrate ~20 field accesses
  - `crates/infrastructure/tests/task_repository_test.rs` — migrate ~25 test assertions
  - `crates/api/src/http/handlers/tasks.rs` — migrate TaskResponse conversion
  - `crates/api/src/cli/output.rs` — migrate CLI formatting
  - `crates/api/src/lib.rs` — migrate tracing/emit accesses
  - `crates/api/examples/basic_enqueue.rs` — migrate example code
  - `crates/api/examples/axum_integration.rs` — migrate example code
  - `crates/api/tests/` (8 files) — migrate test assertions
- **No new files created**
- **No schema changes, no `.sqlx/` regeneration needed**

### Out of Scope

- **`payload` type change to `Arc<serde_json::Value>`** — Story 7.2 (CR20). This story provides the accessor that makes that change transparent.
- **`IronDeferBuilder` changes** — already a builder, no visibility changes needed.
- **`WorkerService` field visibility** — not in public API, not in scope.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 512–536] — Story 6.10 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 930–939] — Public API boundary
- [Source: `crates/domain/src/model/task.rs:77–92`] — TaskRecord struct (all pub fields)
- [Source: `crates/domain/src/model/task.rs:191–208`] — TaskContext struct (all pub fields)
- [Source: `crates/api/src/http/handlers/tasks.rs:70–89`] — From<TaskRecord> for TaskResponse
- [Source: `crates/api/src/lib.rs:70–86`] — Public API re-exports
- [Source: `crates/domain/src/model/queue.rs:12–14`] — QueueName (already encapsulated — model)
- [Source: `crates/domain/src/model/task.rs:21–48`] — TaskId (already encapsulated — model)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

- ✅ Added 14 accessor methods + `into_payload()`, `take_payload()`, `with_status()`, `with_payload()` to TaskRecord
- ✅ Added 3 accessor methods to TaskContext (`task_id()`, `worker_id()`, `attempt()`)
- ✅ Changed all TaskRecord and TaskContext fields from `pub` to `pub(crate)`
- ✅ Migrated ~60+ direct field accesses in worker.rs (dispatch_task, emit functions, poll loop, tests)
- ✅ Migrated ~10 field accesses in scheduler.rs tests
- ✅ Migrated save() INSERT query + #[instrument] in postgres_task_repository.rs with local variable bindings to avoid temporary lifetime issues
- ✅ Migrated ~25 field assertions in task_repository_test.rs
- ✅ Migrated From<TaskRecord> for TaskResponse using consuming into_payload() pattern
- ✅ Migrated CLI output formatting in output.rs
- ✅ Migrated tracing/emit functions in lib.rs (task_enqueued, task_cancelled)
- ✅ Migrated examples (basic_enqueue.rs, axum_integration.rs)
- ✅ Migrated 15+ api test files via sed batch replacement
- ✅ All tests pass, clippy pedantic clean, formatting clean

### Change Log

- 2026-04-23: Story 6.10 implemented — private fields with typed accessors for TaskRecord and TaskContext

### File List

- `crates/domain/src/model/task.rs` — accessor methods, field visibility change, transformation helpers
- `crates/application/src/services/worker.rs` — migrated ~60 field accesses to accessors
- `crates/application/src/services/scheduler.rs` — migrated test assertions to accessors
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — migrated save() query + inline test assertions
- `crates/infrastructure/tests/task_repository_test.rs` — migrated ~25 test assertions
- `crates/api/src/http/handlers/tasks.rs` — migrated From<TaskRecord> for TaskResponse
- `crates/api/src/cli/output.rs` — migrated CLI formatting
- `crates/api/src/lib.rs` — migrated emit_task_enqueued, cancel, get
- `crates/api/examples/basic_enqueue.rs` — migrated field accesses
- `crates/api/examples/axum_integration.rs` — migrated field accesses
- `crates/api/tests/integration_test.rs` — migrated field assertions
- `crates/api/tests/worker_pool_test.rs` — migrated field assertions + TaskContext.attempt
- `crates/api/tests/sweeper_test.rs` — migrated field assertions
- `crates/api/tests/sweeper_counter_test.rs` — migrated field assertions
- `crates/api/tests/shutdown_test.rs` — migrated field assertions
- `crates/api/tests/chaos_max_retries_test.rs` — migrated field assertions
- `crates/api/tests/chaos_worker_crash_test.rs` — migrated field assertions
- `crates/api/tests/audit_trail_test.rs` — migrated field assertions
- `crates/api/tests/lifecycle_log_test.rs` — migrated field assertions
- `crates/api/tests/metrics_test.rs` — migrated field assertions
- `crates/api/tests/otel_lifecycle_test.rs` — migrated field assertions + TaskContext.attempt
- `crates/api/tests/otel_counters_test.rs` — migrated field assertions
- `crates/api/tests/rest_api_test.rs` — migrated field assertions
- `crates/api/tests/cli_test.rs` — migrated field assertions
- `crates/api/tests/common/otel.rs` — migrated field assertions

### Review Findings

- [x] [Review][Patch] Payload cleared by take_payload() before lifecycle logging [crates/application/src/services/worker.rs:492]
