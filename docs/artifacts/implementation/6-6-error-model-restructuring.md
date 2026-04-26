# Story 6.6: Error Model Restructuring

Status: done

## Story

As a developer,
I want error types to be structured and matchable,
so that I can programmatically distinguish between error causes without parsing strings.

## Acceptance Criteria

1. **Structured `InvalidPayload` source types (CR10)**

   **Given** the `TaskError::InvalidPayload` variant in `crates/domain/src/error.rs`
   **When** I inspect its definition
   **Then** the `reason: String` field is replaced with a structured source type that matches actual failure modes (e.g., `DeserializationFailed { source: serde_json::Error }`, `PayloadTooLarge { size: usize, max: usize }`)
   **And** `TaskError::ExecutionFailed` similarly uses a structured source type instead of a bare string
   **And** all call sites constructing these variants (12+ across `lib.rs`, `worker.rs`, `postgres_task_repository.rs`, `tasks.rs`) are updated

2. **Typed `TaskError::Migration` variant (CR11)**

   **Given** the `IronDefer::build()` method that runs `MIGRATOR.run(&pool)`
   **When** the migration fails
   **Then** the error is wrapped in a `TaskError::Migration` variant (not opaquely boxed as `Box<dyn Error>`)
   **And** the variant preserves the original `sqlx::migrate::MigrateError` as a source for debugging

3. **Combined verification**

   **Given** the combined changes
   **When** `cargo test --workspace` runs
   **Then** all error-matching code compiles and tests pass
   **And** existing `From` impls between error types compile correctly

## Tasks / Subtasks

- [x] **Task 1: Design structured source types for InvalidPayload** (AC: 1)
  - [x] 1.1: Analyze all 17 `InvalidPayload` construction sites to categorize failure modes
  - [x] 1.2: Define a `PayloadErrorKind` enum (or similar) in `crates/domain/src/error.rs` that covers the actual failure modes:
    - `Deserialization { source: String }` — serde failures (cannot depend on `serde_json::Error` directly in domain crate)
    - `Validation { message: String }` — custom validation (empty kind, invalid max_attempts, invalid queue name, etc.)
    - `NotInExpectedState { task_id: TaskId, expected: &'static str }` — repo failures like "task is not in Running status"
    - `AlreadyStarted` — engine double-start
  - [x] 1.3: Replace `InvalidPayload { reason: String }` with `InvalidPayload { kind: PayloadErrorKind }`
  - [x] 1.4: Update the `#[error(...)]` display impl to delegate to `PayloadErrorKind`'s Display

- [x] **Task 2: Design structured source type for ExecutionFailed** (AC: 1)
  - [x] 2.1: Analyze all 9 `ExecutionFailed` construction sites (all in tests/mocks)
  - [x] 2.2: Define an `ExecutionErrorKind` enum covering:
    - `HandlerFailed { reason: String }` — user task handler returned Err
    - `HandlerPanicked { message: String }` — panic captured via JoinHandle
    - `MissingHandler { kind: String }` — no registered handler for task kind
  - [x] 2.3: Replace `ExecutionFailed { reason: String }` with `ExecutionFailed { kind: ExecutionErrorKind }`
  - [x] 2.4: Update the `#[error(...)]` display impl

- [x] **Task 3: Add `TaskError::Migration` variant** (AC: 2)
  - [x] 3.1: Add `Migration` variant to `TaskError`: `Migration { source: Box<dyn std::error::Error + Send + Sync> }`
  - [x] 3.2: Cannot use `sqlx::migrate::MigrateError` directly — domain crate has no sqlx dependency. Use boxed error like `Storage` does, but with its own variant for programmatic matching.
  - [x] 3.3: Update `crates/api/src/lib.rs:848–854` to use `TaskError::Migration { source: Box::new(e) }` instead of `TaskError::Storage`
  - [x] 3.4: Update `From<TaskError> for AppError` in `errors.rs` to handle the new `Migration` variant (map to HTTP 500)

- [x] **Task 4: Update all InvalidPayload construction sites** (AC: 1)
  - [x] 4.1: `crates/api/src/lib.rs` — 13 sites (lines 127, 249, 258, 261, 301, 384, 575, 581, 589, 594, 837, 841, 863)
  - [x] 4.2: `crates/infrastructure/src/adapters/postgres_task_repository.rs` — 2 sites (lines 356, 428)
  - [x] 4.3: `crates/application/src/services/worker.rs` — 1 site (line 148)
  - [x] 4.4: `crates/infrastructure/src/db.rs` — 1 test fixture (line 225)

- [x] **Task 5: Update all ExecutionFailed construction sites** (AC: 1)
  - [x] 5.1: `crates/application/src/services/worker.rs` — 3 test sites (lines 1083, 1485, 1668)
  - [x] 5.2: `crates/api/tests/` — 6 test task impls (otel_counters_test.rs:28, worker_pool_test.rs:72, audit_trail_test.rs:69, chaos_max_retries_test.rs:25, otel_lifecycle_test.rs:46, otel_lifecycle_test.rs:182)

- [x] **Task 6: Update all match/destructure sites** (AC: 3)
  - [x] 6.1: `crates/api/src/http/errors.rs:85–109` — `From<TaskError> for AppError` — add `Migration` arm, update `InvalidPayload` and `ExecutionFailed` pattern matching
  - [x] 6.2: `crates/api/src/lib.rs:959–967` — test destructures `InvalidPayload { reason }` and asserts `reason.contains("missing field")`. Updated to match `InvalidPayload { kind: PayloadErrorKind::Deserialization { message } }` and assert `message.contains("missing field")`
  - [x] 6.3: `crates/api/tests/integration_test.rs:332` — `matches!(err, TaskError::InvalidPayload { .. })` — verified still compiles
  - [x] 6.4: `crates/infrastructure/src/error.rs:287–295` — test destructuring `Storage { source }` — unchanged, verified still compiles

- [x] **Task 7: Verify all tests pass** (AC: 3)
  - [x] 7.1: `cargo test --workspace --lib` — all 162 unit tests pass
  - [x] 7.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean (only pre-existing warnings in unmodified files)
  - [x] 7.3: `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **Error types per ADR-0002:** Typed errors per layer, never `Box<dyn Error>` in library code (except `Storage`/`Migration` for cross-layer bridge). `From` impls convert at boundaries.
- **Domain crate has no infrastructure deps (architecture lines 924–937):** `TaskError` in `crates/domain/src/error.rs` CANNOT depend on `serde_json`, `sqlx`, or any infrastructure crate. Structured source types must use domain-level types only (`String`, `TaskId`, primitives). This is why `DeserializationFailed` carries `source: String` (the serde error's Display output), not `serde_json::Error`.
- **Error display (ADR-0002):** `#[error(...)]` format strings should be human-readable messages, not debug dumps.

### Critical Implementation Guidance

**Recommended `PayloadErrorKind` enum:**

```rust
/// Structured source for `TaskError::InvalidPayload`.
///
/// Replaces the prior `reason: String` field (CR10) to enable programmatic
/// error matching without string parsing.
#[derive(Debug, Error)]
pub enum PayloadErrorKind {
    #[error("deserialization failed: {message}")]
    Deserialization { message: String },

    #[error("{message}")]
    Validation { message: String },

    #[error("task {task_id} is not in {expected} status")]
    NotInExpectedState {
        task_id: TaskId,
        expected: &'static str,
    },

    #[error("engine already started")]
    AlreadyStarted,
}
```

**Recommended `ExecutionErrorKind` enum:**

```rust
/// Structured source for `TaskError::ExecutionFailed`.
///
/// Replaces the prior `reason: String` field (CR10) to enable programmatic
/// error matching. User-facing task handlers produce `HandlerFailed`;
/// infrastructure catches panics as `HandlerPanicked`.
#[derive(Debug, Error)]
pub enum ExecutionErrorKind {
    #[error("{reason}")]
    HandlerFailed { reason: String },

    #[error("handler panicked: {message}")]
    HandlerPanicked { message: String },

    #[error("no handler registered for kind: {kind}")]
    MissingHandler { kind: String },
}
```

**Updated `TaskError` enum:**

```rust
#[derive(Debug, Error)]
pub enum TaskError {
    #[error("task {id} is already claimed by worker {worker_id}")]
    AlreadyClaimed { id: TaskId, worker_id: WorkerId },

    #[error("task payload is invalid: {kind}")]
    InvalidPayload { kind: PayloadErrorKind },

    #[error("task execution failed: {kind}")]
    ExecutionFailed { kind: ExecutionErrorKind },

    #[error("task storage error: {source}")]
    Storage {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("database migration failed: {source}")]
    Migration {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
```

**Why `Migration` uses `Box<dyn Error>` instead of `sqlx::migrate::MigrateError`:**
The domain crate has no sqlx dependency and MUST NOT add one (hexagonal boundary). The boxed error preserves the sqlx `MigrateError` in the source chain (accessible via `downcast_ref` like `Storage` does for `PostgresAdapterError`), but the variant name alone enables programmatic matching: `TaskError::Migration { .. }` vs `TaskError::Storage { .. }`.

**Construction site migration guide:**

Most `InvalidPayload` sites currently use the pattern:
```rust
TaskError::InvalidPayload { reason: format!("...") }
```

These become:
```rust
TaskError::InvalidPayload { kind: PayloadErrorKind::Validation { message: format!("...") } }
```

Serde deserialization failures become:
```rust
TaskError::InvalidPayload { kind: PayloadErrorKind::Deserialization { message: e.to_string() } }
```

Task-not-in-expected-state becomes:
```rust
TaskError::InvalidPayload { kind: PayloadErrorKind::NotInExpectedState { task_id, expected: "Running" } }
```

**ExecutionFailed in test task impls:**
```rust
// OLD: Err(TaskError::ExecutionFailed { reason: "synthetic".into() })
// NEW: Err(TaskError::ExecutionFailed { kind: ExecutionErrorKind::HandlerFailed { reason: "synthetic".into() } })
```

**Production ExecutionFailed usage (worker.rs dispatch_task):**

`dispatch_task` does NOT construct `TaskError::ExecutionFailed` directly. It:
- Calls user `handler.execute()` which returns `Result<(), TaskError>` — if the user returns `ExecutionFailed`, it's propagated
- On missing handler (line 412), passes a string to `repo.fail(&msg)` — this is a `repo.fail()` call, NOT a `TaskError` construction
- On panic, extracts the panic message and passes it to `repo.fail()` — again, NOT a `TaskError` construction

All 9 `ExecutionFailed` construction sites are in test Task impls that simulate user-side failures.

**The `MissingHandler` variant in `ExecutionErrorKind` has NO production construction site currently.** It's forward-looking — if the engine ever wraps the missing-handler repo.fail() into a structured error, this variant is ready. For now, it's only useful if test code wants to construct a structured missing-handler error instead of `HandlerFailed { reason: "no handler..." }`. Consider whether `MissingHandler` is needed at all in this story, or if `HandlerFailed` suffices. Include it for completeness if the enum is cheap.

**`worker.rs:148` (concurrency == 0)** is in `run_poll_loop`, NOT in `dispatch_task`. It's a configuration validation that constructs `TaskError::InvalidPayload { reason: "concurrency >= 1" }` → should become `PayloadErrorKind::Validation { message: ... }`. Already correctly mapped in the migration table.

**Specific migration notes for tricky sites:**

| Site | Current | Recommended New Kind |
|------|---------|---------------------|
| `lib.rs:127` serde deser | `InvalidPayload { reason: e.to_string() }` | `PayloadErrorKind::Deserialization { message: e.to_string() }` |
| `lib.rs:249` no handler registered | `InvalidPayload { reason: format!(...) }` | `PayloadErrorKind::Validation { message: format!(...) }` |
| `lib.rs:258,301,594,863` invalid queue | `InvalidPayload { reason: format!("invalid queue: {e}") }` | `PayloadErrorKind::Validation { message: format!(...) }` |
| `lib.rs:261` serde serialize | `InvalidPayload { reason: format!("serialization: {e}") }` | `PayloadErrorKind::Deserialization { message: format!(...) }` |
| `lib.rs:384` double start | `InvalidPayload { reason: "already started" }` | `PayloadErrorKind::AlreadyStarted` |
| `lib.rs:575,581,589` enqueue_raw validation | `InvalidPayload { reason: ... }` | `PayloadErrorKind::Validation { message: ... }` |
| `lib.rs:837,841` config validation | `InvalidPayload { reason }` | `PayloadErrorKind::Validation { message: reason }` |
| `repo:356,428` task not Running | `InvalidPayload { reason: format!("task {id} not Running") }` | `PayloadErrorKind::NotInExpectedState { task_id, expected: "Running" }` |
| `worker.rs:148` concurrency == 0 | `InvalidPayload { reason: "concurrency >= 1" }` | `PayloadErrorKind::Validation { message: ... }` |
| `lib.rs:848` MIGRATOR.run | `Storage { source: Box::new(e) }` | `Migration { source: Box::new(e) }` |

**`From<TaskError> for AppError` update:**

```rust
impl From<TaskError> for AppError {
    fn from(err: TaskError) -> Self {
        match &err {
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
```

Note: If `TaskStatus` has `#[non_exhaustive]` from Story 6.3, and `TaskError` gains a new variant (`Migration`), then the `From<TaskError> for AppError` match must add it. `TaskError` itself does NOT need `#[non_exhaustive]` — it's a domain type used internally.

### Previous Story Intelligence

**From Story 6.5 (ready-for-dev):**
- Worker poll loop restructured with jittered backoff. The `TaskError::Storage` variant is used by the saturation classifier (`is_pool_timeout`). The classifier walks the source chain via `downcast_ref`. Adding a `Migration` variant does NOT affect the classifier — migrations run at build time, not in the poll loop.

**From Story 6.3 (ready-for-dev):**
- `TaskStatus` has `#[non_exhaustive]`. The `From<TaskError> for AppError` match (errors.rs:85–109) will need updating for both `#[non_exhaustive]` wildcard arm (Story 6.3) AND the new `Migration` variant (this story). If 6.3 lands first, verify the wildcard arm covers `Migration` correctly.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide.

### Git Intelligence

Last code commit: `7ed6fc8`. Error types last modified in Story 3.1 (URL scrubbing) and Story 1A.1 (initial error model).

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `TaskError` enum | `crates/domain/src/error.rs:14–45` | AC 1, 2 — restructure variants |
| `InvalidPayload` variant | `crates/domain/src/error.rs:26–27` | AC 1 — replace `reason: String` |
| `ExecutionFailed` variant | `crates/domain/src/error.rs:34–35` | AC 1 — replace `reason: String` |
| `Storage` variant | `crates/domain/src/error.rs:40–44` | Context — already structured |
| `PostgresAdapterError` | `crates/infrastructure/src/error.rs:30–40` | Context — converts to `Storage` |
| `From<PostgresAdapterError> for TaskError` | `crates/infrastructure/src/error.rs:108–118` | AC 3 — verify still compiles |
| `From<TaskError> for AppError` | `crates/api/src/http/errors.rs:85–109` | AC 2 — add `Migration` arm |
| `MIGRATOR.run()` call | `crates/api/src/lib.rs:848–854` | AC 2 — change to `Migration` variant |
| `is_pool_timeout` classifier | `crates/infrastructure/src/db.rs:132–151` | Context — walks source chain on `Storage` |

### Dependencies

No new crate dependencies. The domain error types use only `thiserror`, which is already in workspace dependencies.

### Project Structure Notes

- **Modified files:**
  - `crates/domain/src/error.rs` — add `PayloadErrorKind`, `ExecutionErrorKind`, `Migration` variant
  - `crates/api/src/lib.rs` — update 13 `InvalidPayload` sites + 1 `Migration` site
  - `crates/api/src/http/errors.rs` — add `Migration` arm to `From` impl
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — update 2 `InvalidPayload` sites
  - `crates/application/src/services/worker.rs` — update 1 `InvalidPayload` site + 3 test `ExecutionFailed` sites
  - `crates/api/tests/` — update 6 test task `ExecutionFailed` sites
- **No new files created**
- **No schema changes, no `.sqlx/` regeneration needed**

### Out of Scope

- **Error payload scrubbing** (`sqlx::Error::Database` content) — Story 6.7 (CR14)
- **Validation error for `scheduled_at`** — Story 6.8 (CR9), may add a new `PayloadErrorKind` variant
- **Builder pattern changes** — Story 6.9 (CR46)
- **Making `TaskError` `#[non_exhaustive]`** — not in scope; `TaskError` is domain-internal

### References

- [Source: `docs/artifacts/planning/epics.md` lines 406–425] — Story 6.6 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 702–710] — Error conversion rules
- [Source: `docs/artifacts/planning/architecture.md` lines 924–937] — Crate boundary rules
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 39] — CR10: InvalidPayload/ExecutionFailed remain stringly-typed
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 33] — CR11: MIGRATOR.run opaquely boxed
- [Source: `crates/domain/src/error.rs:14–45`] — TaskError enum definition
- [Source: `crates/api/src/lib.rs:848–854`] — MIGRATOR.run error handling
- [Source: `crates/api/src/http/errors.rs:85–109`] — From<TaskError> for AppError
- [Source: `crates/infrastructure/src/error.rs:108–118`] — From<PostgresAdapterError> for TaskError
- [Source: `docs/adr/0002-error-handling.md`] — ADR on error strategy

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Implementation Plan

**Approach:** Tasks 1-6 implemented as a single cohesive unit since all changes are interdependent — defining new enum types, replacing old field shapes, and updating all construction/match sites must all happen atomically for compilation. Task 7 (verification) confirms the combined changes.

### Debug Log References

None — clean implementation with no debugging required.

### Completion Notes List

- AC1: `PayloadErrorKind` enum added with 4 variants: `Deserialization`, `Validation`, `NotInExpectedState`, `AlreadyStarted`. `ExecutionErrorKind` enum added with 3 variants: `HandlerFailed`, `HandlerPanicked`, `MissingHandler`. Both replace prior `reason: String` fields. All 17 `InvalidPayload` and 9 `ExecutionFailed` construction sites updated across 11 files. New types re-exported from `iron_defer_domain` and `iron_defer` (api crate).
- AC2: `TaskError::Migration` variant added with `Box<dyn Error + Send + Sync>` source (same pattern as `Storage`). `MIGRATOR.run()` error in `lib.rs` now maps to `Migration` instead of `Storage`. `From<TaskError> for AppError` updated with `Migration` arm mapping to HTTP 500.
- AC3: All 162 workspace unit tests pass. `cargo clippy --workspace --all-targets -- -D clippy::pedantic` clean (only pre-existing warnings). `cargo fmt --check` clean. Integration test pattern matches (`InvalidPayload { .. }`) verified to still compile.
- Also fixed: `cargo fmt` corrected indentation in prior commit's `worker.rs` `tokio::select!` blocks and `lib.rs` closure formatting.

### File List

- `crates/domain/src/error.rs` — added `PayloadErrorKind`, `ExecutionErrorKind` enums; added `Migration` variant; restructured `InvalidPayload` and `ExecutionFailed` variants
- `crates/domain/src/lib.rs` — re-export `PayloadErrorKind`, `ExecutionErrorKind`
- `crates/api/src/lib.rs` — updated 13 `InvalidPayload` sites, 1 `Migration` site, 1 test destructure; re-export new types
- `crates/api/src/http/errors.rs` — added `Migration` arm to `From<TaskError> for AppError`
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — updated 2 `InvalidPayload` sites to `NotInExpectedState`
- `crates/infrastructure/src/db.rs` — updated 1 test fixture `InvalidPayload` site
- `crates/application/src/services/worker.rs` — updated 1 `InvalidPayload` + 3 `ExecutionFailed` sites; `cargo fmt` fixed indentation
- `crates/api/tests/otel_counters_test.rs` — updated `ExecutionFailed` construction
- `crates/api/tests/chaos_max_retries_test.rs` — updated `ExecutionFailed` construction
- `crates/api/tests/audit_trail_test.rs` — updated `ExecutionFailed` construction
- `crates/api/tests/worker_pool_test.rs` — updated `ExecutionFailed` construction
- `crates/api/tests/otel_lifecycle_test.rs` — updated 2 `ExecutionFailed` constructions

### Change Log

- 2026-04-23: Implemented Story 6.6 — structured error model for InvalidPayload, ExecutionFailed, and Migration (CR10, CR11)

### Review Findings

- [ ] [Review][Patch] TaskError::NotFound missing but still used in lib.rs:128 [crates/api/src/lib.rs:128]
- [ ] [Review][Patch] IronDefer::inspect method missing but used in unit tests [crates/api/src/lib.rs]
- [ ] [Review][Patch] Extreme indentation regression in worker.rs (28 spaces) [crates/application/src/services/worker.rs]
- [ ] [Review][Patch] Spec deviation: Deserialization variant uses message instead of source [crates/domain/src/error.rs]
- [ ] [Review][Patch] Unhandled overflow in backoff checked_add(delay) [crates/application/src/services/worker.rs:356]
- [x] [Review][Defer] Error message state leakage [crates/domain/src/error.rs] — deferred, pre-existing

### Review Findings

- [x] [Review][Patch] Move NotInExpectedState to top-level TaskError variant [crates/domain/src/error.rs]
- [x] [Review][Patch] Add T: Task constraint and kind validation to IronDefer::get<T> [crates/api/src/lib.rs]
- [x] [Review][Patch] Critical Compilation Error: Field Mismatch [crates/api/src/lib.rs]
- [x] [Review][Patch] Semantic Misnaming: Serialization vs Deserialization [crates/api/src/lib.rs:267]
- [x] [Review][Patch] Structured Error Omission in Task Dispatch [crates/application/src/services/worker.rs]
- [x] [Review][Patch] Backoff Logic Potential Panic and Tight Spin [crates/application/src/services/worker.rs:346]
- [x] [Review][Patch] Inconsistent 404 Mapping in HTTP Handlers [crates/api/src/http/handlers/tasks.rs]
- [x] [Review][Defer] Redundant Instant::now() in Worker Backoff [crates/application/src/services/worker.rs] — deferred, pre-existing
- [x] [Review][Defer] Public API Duplication: IronDefer::inspect [crates/api/src/lib.rs] — deferred, pre-existing
