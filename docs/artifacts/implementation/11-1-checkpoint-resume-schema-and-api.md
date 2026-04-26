# Story 11.1: Checkpoint/Resume Schema & API

Status: done

## Story

As a developer building multi-step workflows,
I want to persist checkpoint data during execution and resume from the last checkpoint after a crash,
so that a failure at step 4 of 5 doesn't waste the work from steps 1-3.

## Acceptance Criteria

1. **Given** a task handler calling `ctx.checkpoint(data)`
   **When** the checkpoint is persisted
   **Then** the `checkpoint` JSONB column on the tasks row is updated atomically

2. **Given** a task that crashed after checkpointing at step 3
   **When** the sweeper recovers the task and a worker retries it
   **Then** `ctx.last_checkpoint()` returns the step 3 checkpoint data

3. **Given** a task that has no prior checkpoint
   **When** `ctx.last_checkpoint()` is called on the first attempt
   **Then** `None` is returned

4. **Given** a task that completes successfully
   **When** completion is recorded
   **Then** checkpoint data is cleared (set to NULL)

5. **Given** the REST API
   **When** `GET /tasks/{id}` is called for a task with checkpoint data
   **Then** the response includes a `lastCheckpoint` field (nullable)

## Functional Requirements Coverage

- **FR57:** Task handler persists checkpoint data via TaskContext, recoverable on retry
- **FR58:** On retry after crash, task handler retrieves most recent checkpoint data
- **FR59:** Checkpoint data cleared automatically on task completion

## Tasks / Subtasks

- [x] Task 1: Database migration (AC: 1)
  - [x] 1.1 Create `migrations/0009_add_checkpoint_column.sql` — `ALTER TABLE tasks ADD COLUMN checkpoint JSONB;`
  - [x] 1.2 Regenerate `.sqlx/` offline cache after migration

- [x] Task 2: Domain model changes (AC: 1, 2, 3, 5)
  - [x] 2.1 Add `checkpoint: Option<serde_json::Value>` field to `TaskRecord`
  - [x] 2.2 Add `pub fn checkpoint(&self) -> Option<&serde_json::Value>` accessor method
  - [x] 2.3 Consuming accessor not needed — `checkpoint().cloned()` used in TaskResponse
  - [x] 2.4 bon `maybe_checkpoint` used via `Option<T>` pattern (defaults to `None`)

- [x] Task 3: TaskContext checkpoint API (AC: 1, 2, 3)
  - [x] 3.1 Removed `PartialEq, Eq` derives from `TaskContext`, added manual `Debug` impl
  - [x] 3.2 Added `last_checkpoint: Option<serde_json::Value>` field to `TaskContext`
  - [x] 3.3 Used `Arc<dyn CheckpointWriter>` trait instead of raw `PgPool` (hexagonal compliance)
  - [x] 3.4 `task_id` is `Copy` — no clone needed
  - [x] 3.5 Implemented `pub async fn checkpoint(&self, data: serde_json::Value) -> Result<(), TaskError>`
  - [x] 3.6 Implemented `pub fn last_checkpoint(&self) -> Option<&serde_json::Value>`
  - [x] 3.7 Added `with_checkpoint()` builder method; `TaskContext::new()` backwards-compatible
  - [x] 3.8 Checkpoint size validation: rejects payloads exceeding 1 MiB

- [x] Task 4: Infrastructure — repository layer (AC: 1, 2, 4)
  - [x] 4.1–4.12 All TaskRow, TaskRowWithTotal, From/TryFrom impls, INSERT/UPDATE/SELECT/RETURNING clauses updated
  - [x] 4.8 `complete()` sets `checkpoint = NULL`
  - [x] PostgresCheckpointWriter implemented and exported

- [x] Task 5: Worker dispatch — supply checkpoint to TaskContext (AC: 2, 3)
  - [x] 5.1 Added `checkpoint_writer: Option<Arc<dyn CheckpointWriter>>` to `DispatchContext` and `WorkerService`
  - [x] 5.2 `dispatch_task()` calls `with_checkpoint()` when writer is available
  - [x] 5.3 First attempt with no checkpoint returns `None` — no special handling needed

- [x] Task 6: REST API response (AC: 5)
  - [x] 6.1 Added `last_checkpoint: Option<serde_json::Value>` to `TaskResponse`
  - [x] 6.2 Serializes as `lastCheckpoint` via `#[serde(rename_all = "camelCase")]`
  - [x] 6.3 `From<TaskRecord>` maps `checkpoint()` → `last_checkpoint`

- [x] Task 7: Integration tests (AC: 1-5)
  - [x] 7.1 `checkpoint_persists_and_survives_retry` — passes
  - [x] 7.2 `checkpoint_none_on_first_attempt` — passes
  - [x] 7.3 `checkpoint_cleared_on_completion` — passes
  - [x] 7.4 `checkpoint_visible_in_rest_api` — passes
  - [x] 7.5 `checkpoint_size_limit` — passes
  - [x] 7.6 `checkpoint_multiple_overwrites` — passes

- [x] Task 8: Offline cache & compilation (AC: all)
  - [x] 8.1 `.sqlx/` offline cache regenerated
  - [x] 8.2 `cargo test --workspace` passes (pre-existing traces test failure unrelated)
  - [x] 8.3 `cargo clippy --workspace` clean (only pre-existing warnings)

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules:**
- Migration: `migrations/0009_add_checkpoint_column.sql`
- Domain: `TaskRecord` + `TaskContext` in `crates/domain/src/model/task.rs` — no DB deps, just data
- Infrastructure: `PostgresTaskRepository` maps `checkpoint` column to/from domain types
- Application: Worker dispatch threads checkpoint through `DispatchContext` → `TaskContext`
- API: `TaskResponse` DTO adds `lastCheckpoint` field, `From<TaskRecord>` maps it

**`TaskContext` is `#[non_exhaustive]` (line 272):** Adding fields is a non-breaking change. The architecture comment at line 264 specifically anticipates this. **However:** `TaskContext` currently derives `PartialEq, Eq` (line 271). `PgPool` does NOT implement `Eq`, so these derives MUST be removed before adding the `pool` field. Grep for `TaskContext` equality comparisons and remove any.

### Critical Implementation Details

1. **Single DB round-trip per checkpoint:** `ctx.checkpoint()` executes `UPDATE tasks SET checkpoint = $1, updated_at = now() WHERE id = $2`. No transaction wrapper needed — it's a single atomic UPDATE.

2. **PgPool in TaskContext:** The worker dispatch already has access to the pool via `DispatchContext`. Pass `pool.clone()` (PgPool is Arc-wrapped internally). Do NOT use the SKIP LOCKED connection — checkpoint writes must use a separate connection from the pool to avoid holding the claim lock during the write.

3. **Checkpoint clearing on completion:** The `complete()` method in `postgres_task_repository.rs` (~line 539) must add `checkpoint = NULL` to its UPDATE SET clause. This is a single-query change.

4. **RETURNING checkpoint in claim_next:** The `claim_next()` query (~line 492) uses `FOR UPDATE SKIP LOCKED` and then updates. The RETURNING clause must include `checkpoint` so the worker has the last checkpoint value when constructing `TaskContext`.

5. **Audit log interaction:** If `audit_log = true`, checkpoint writes are NOT audited (checkpoints are within-execution state, not state transitions). Only the completion clearing is audited (as part of the Running→Completed transition, which already has audit support).

6. **Error type for checkpoint:** Use existing `TaskError::ExecutionFailed` with a new `ExecutionErrorKind` variant or reuse `DatabaseError`. Checkpoint failures during execution should propagate to the handler — the handler decides whether to retry or continue without checkpoint.

7. **1 MiB size limit:** Check `data.to_string().len() > 1_048_576` before the UPDATE. This is consistent with the existing payload size limit. Return a clear error: `"checkpoint data exceeds 1 MiB limit"`.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `migrations/0009_add_checkpoint_column.sql` | **NEW** — `ALTER TABLE tasks ADD COLUMN checkpoint JSONB;` |
| `crates/domain/src/model/task.rs` | Add `checkpoint` field to `TaskRecord`, `last_checkpoint` + `pool` to `TaskContext`, new methods |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Add `checkpoint` to `TaskRow`, all INSERT/UPDATE/SELECT queries, RETURNING clauses |
| `crates/application/src/services/worker.rs` | Pass `last_checkpoint` and `pool` when constructing `TaskContext` in `dispatch_task()` |
| `crates/api/src/http/handlers/tasks.rs` | Add `last_checkpoint` to `TaskResponse`, update `From<TaskRecord>` |
| `crates/api/tests/checkpoint_test.rs` | **NEW** — integration tests |
| `.sqlx/` | Regenerate offline cache |

### Testing Standards

- Integration tests in `crates/api/tests/checkpoint_test.rs` as flat file
- Use `fresh_pool_on_shared_container()` for clean DB state per test
- Unique queue names per test for isolation
- Skip gracefully when Docker is unavailable
- Create a `CheckpointTask` that calls `ctx.checkpoint()` with test data
- For retry tests: use `RetryCountingTask` pattern from `common/e2e.rs` or create a checkpoint-aware variant that checkpoints on each attempt

### Critical Constraints

1. **Checkpoint writes use a SEPARATE pool connection**, not the worker's claimed connection. The SKIP LOCKED claim holds a row-level lock — writing checkpoint on the same connection would work, but using a separate connection ensures the checkpoint write doesn't block if the claim connection is slow.

2. **`updated_at` must be set on checkpoint writes** — `SET checkpoint = $1, updated_at = now()`. This keeps the `updated_at` timestamp accurate for monitoring.

3. **NFR-R9: < 50ms at p99 for 1 MiB checkpoints.** This is verified in Story 11.3 benchmarks, not here. But the implementation must be efficient enough to meet this target (single UPDATE, no transaction overhead).

4. **G7 (HITL) depends on this:** Story 12.1 `ctx.suspend()` will call `ctx.checkpoint()` internally before releasing the worker slot. The checkpoint API must be stable and tested before Epic 12 begins.

5. **`.sqlx/` offline cache:** Must be regenerated after adding the checkpoint column to all queries. Run `cargo sqlx prepare --workspace`.

### Previous Story Intelligence

**From Story 10.2 (audit log — previous epic, closest pattern):**
- Adding a column to tasks table follows the same migration → TaskRow → TaskRecord → queries pattern
- All RETURNING clauses needed updating when `trace_id` was added — same pattern for `checkpoint`
- `complete()` already wraps in transaction for audit — adding `checkpoint = NULL` to the SET clause is trivial
- `save_in_tx()` pattern from Story 9.2 handles caller-provided transactions — checkpoint column defaults to NULL

**From Story 10.1 (OTel traces — column addition pattern):**
- `trace_id` was added as `Option<String>` with accessor — same pattern for `checkpoint` as `Option<serde_json::Value>`
- Migration was a simple `ALTER TABLE ADD COLUMN` — identical approach

**From Story 10.3 (E2E tests):**
- `RetryCountingTask` in `common/e2e.rs` uses `ctx.attempt().get()` — checkpoint variant can use the same mechanism
- `boot_e2e_engine()` pattern is stable and ready for checkpoint tests

### Existing Infrastructure to Reuse

- `TaskError` variants — reuse `ExecutionFailed` for checkpoint size limit errors
- `PostgresAdapterError::from(sqlx::Error)` — reuse for checkpoint DB errors
- `bon::Builder` on `TaskRecord` — new `Option<>` field defaults to `None` automatically
- `#[non_exhaustive]` on `TaskContext` — adding fields is safe

### References

- [Source: docs/artifacts/planning/epics.md — Epic 11, Story 11.1 (lines 1146-1179)]
- [Source: docs/artifacts/planning/prd.md — FR57, FR58, FR59 (lines 983-985)]
- [Source: docs/artifacts/planning/prd.md — NFR-R9 (line 1055)]
- [Source: docs/artifacts/planning/architecture.md — TaskContext Extension, checkpoint (lines 1862-1878)]
- [Source: docs/artifacts/planning/architecture.md — Schema Evolution G6 (lines 1778-1779)]
- [Source: docs/artifacts/planning/architecture.md — REST API GET /tasks/{id} G6 (line 1924)]
- [Source: crates/domain/src/model/task.rs — TaskRecord (lines 77-97), TaskContext (lines 273-303)]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — save() (~245), claim_next() (~492), complete() (~539)]
- [Source: crates/application/src/services/worker.rs — dispatch_task() (lines 441-750), TaskContext construction (line 447)]
- [Source: crates/api/src/http/handlers/tasks.rs — TaskResponse (lines 56-79), From impl (lines 81-120)]
- [Source: docs/artifacts/implementation/10-2-append-only-audit-log.md — column addition pattern]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

### Review Findings (Group 1: Core & Data Model)

- [x] [Review][Patch] Checkpoint writer lacks worker identity verification [crates/domain/src/model/task.rs:360]
- [x] [Review][Patch] Inefficient double-serialization in checkpoint [crates/domain/src/model/task.rs:353]
- [x] [Review][Patch] Misleading error reporting in checkpoint [crates/domain/src/model/task.rs:356]

### Review Findings (Group 4: Tests & Benchmarks)

- [x] [Review][Patch] Benchmark synchronization weakness in audit_overhead.rs [crates/api/benches/audit_overhead.rs:141]
- [x] [Review][Patch] TestServer shutdown ignores potential failures [crates/api/tests/common/e2e.rs:44]
- [x] [Review][Patch] Repeated global OTel re-initialization [crates/api/tests/otel_traces_test.rs:105]
- [x] [Review][Patch] Hard-coded SQL IDs in audit tests [crates/api/tests/audit_log_test.rs:104]
