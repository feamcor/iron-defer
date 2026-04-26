# Story 1B.1: Atomic Claiming & Task Completion

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a Rust developer,
I want tasks claimed atomically via SKIP LOCKED and their outcomes recorded reliably,
So that no two workers process the same task and every result is persisted.

## Acceptance Criteria

1. **`TaskRepository` port gains three new methods** in `crates/application/src/ports/task_repository.rs`:
   - `async fn claim_next(&self, queue: &QueueName, worker_id: WorkerId, lease_duration: Duration) -> Result<Option<TaskRecord>, TaskError>;`
   - `async fn complete(&self, task_id: TaskId) -> Result<TaskRecord, TaskError>;`
   - `async fn fail(&self, task_id: TaskId, error_message: &str, base_delay_secs: f64, max_delay_secs: f64) -> Result<TaskRecord, TaskError>;`
     > **Note:** The actual implementation uses 4 parameters (adding `base_delay_secs` and `max_delay_secs` for backoff computation) per Dev Notes option 1.
   - All three are added to the existing `#[async_trait]` + `#[automock]` trait. `mockall::automock` automatically generates mock implementations for each.
   - `Duration` is `std::time::Duration` (not `chrono::Duration`). This matches `tokio::time::interval` ergonomics and avoids `chrono` in the port signature.
   - The `fail()` method handles retry-vs-terminal logic **in the SQL** (see AC 4), NOT in Rust application code. The repository is the authority on `attempts` vs `max_attempts` comparison because it reads the current row atomically.

2. **`PostgresTaskRepository::claim_next` implements the Architecture D2.1 atomic query** in `crates/infrastructure/src/adapters/postgres_task_repository.rs`:
   ```sql
   UPDATE tasks
   SET status = 'running',
       claimed_by = $1,
       claimed_until = now() + $2::interval,
       attempts = attempts + 1,
       updated_at = now()
   WHERE id = (
       SELECT id FROM tasks
       WHERE queue = $3
         AND status = 'pending'
         AND scheduled_at <= now()
       ORDER BY priority DESC, scheduled_at ASC
       FOR UPDATE SKIP LOCKED
       LIMIT 1
   )
   RETURNING *;
   ```
   - Uses `sqlx::query_as!` with the `TaskRow` struct and `TryFrom<TaskRow>` for validation at the adapter boundary (same pattern as `save`/`find_by_id`/`list_by_queue`).
   - `lease_duration` passed as a Postgres `INTERVAL` (cast `$2::interval` from `std::time::Duration` seconds as `PgInterval` or use a formatted interval string — see Tooling Notes).
   - Returns `Ok(None)` when no rows match (empty result set = no pending tasks). No error, no block.
   - Decorated with `#[instrument(skip(self), fields(queue = %queue, worker_id = %worker_id), err)]`.
   - The existing `idx_tasks_claiming` partial index (from migration `0001`) covers the subquery: `WHERE queue = $3 AND status = 'pending'` with `ORDER BY priority DESC, scheduled_at ASC`.

3. **`PostgresTaskRepository::complete` atomically transitions a task to `Completed`:**
   ```sql
   UPDATE tasks
   SET status = 'completed',
       updated_at = now()
   WHERE id = $1
     AND status = 'running'
   RETURNING *;
   ```
   - Returns `Err(TaskError::InvalidPayload { reason: "task {id} is not in Running status" })` if no rows updated (task not found or not in `Running` status). Use `InvalidPayload` for now — a more specific variant is deferred to Epic 5 error model review.
   - Decorated with `#[instrument(skip(self), fields(task_id = %task_id), err)]`.

4. **`PostgresTaskRepository::fail` handles retry vs terminal failure atomically in SQL:**
   - **Retry case** (`attempts < max_attempts`): transitions to `Pending`, clears `claimed_by`/`claimed_until`, sets `scheduled_at` per the backoff formula, sets `last_error`.
   - **Terminal case** (`attempts >= max_attempts`): transitions to `Failed` (FR43), sets `last_error`, does NOT clear `claimed_by`/`claimed_until` (preserves last-claimer forensic data).
   - Implementation: TWO separate queries in sequence (not a single CTE), each guarded by `AND status = 'running'`:
     ```sql
     -- Attempt retry first
     UPDATE tasks
     SET status = 'pending',
         claimed_by = NULL,
         claimed_until = NULL,
         last_error = $2,
         scheduled_at = now() + make_interval(secs => LEAST($3 * power(2, attempts - 1), $4)),
         updated_at = now()
     WHERE id = $1
       AND status = 'running'
       AND attempts < max_attempts
     RETURNING *;
     ```
     If zero rows returned:
     ```sql
     -- Terminal failure
     UPDATE tasks
     SET status = 'failed',
         last_error = $2,
         updated_at = now()
     WHERE id = $1
       AND status = 'running'
       AND attempts >= max_attempts
     RETURNING *;
     ```
   - **Backoff formula** (Architecture D1.2): `now() + min(base_delay * 2^(attempts - 1), max_delay)`. Default `base_delay = 5s`, `max_delay = 1800s` (30 min). These are passed as `f64` seconds to the SQL function `make_interval(secs => ...)`.
   - `last_error` is truncated to `LAST_ERROR_MAX_BYTES` (4096) on the write path using the existing `truncate_last_error` helper before binding.
   - Returns `Err(TaskError::InvalidPayload { reason: "task {id} is not in Running status" })` if neither query matches.
   - Decorated with `#[instrument(skip(self, error_message), fields(task_id = %task_id), err)]`.

5. **`TaskExecutor` port trait is redesigned** in `crates/application/src/ports/task_executor.rs`:
   - Replace the current stub:
     ```rust
     #[cfg_attr(test, mockall::automock)]
     #[async_trait]
     pub trait TaskExecutor: Send + Sync + 'static {
         async fn execute(&self, task: &TaskRecord, ctx: &TaskContext) -> Result<(), TaskError>;
     }
     ```
   - The new signature takes `&TaskRecord` (for `kind`, `payload`, `id`) AND `&TaskContext` (for `worker_id`, `attempt`).
   - This resolves the deferred-work item: "`TaskExecutor::execute` lacks `TaskContext` and bridge to `Task::execute`".

6. **New migration `0002_add_claim_check.sql`** adds the cross-field invariant CHECK constraint deferred from Epic 1A:
   ```sql
   ALTER TABLE tasks
       ADD CONSTRAINT tasks_claim_fields_check
       CHECK ((claimed_by IS NULL) = (claimed_until IS NULL));
   ```
   - This enforces that `claimed_by` and `claimed_until` are always both NULL or both non-NULL.
   - This resolves the deferred-work item: "`(claimed_by, claimed_until)` cross-field invariant unguarded".
   - After adding the migration, regenerate the `.sqlx/` offline cache.

7. **`WorkerConfig` gains claiming-related fields** in `crates/application/src/config.rs`:
   - `pub lease_duration: Duration` (default: 300s / 5 min)
   - `pub base_delay: Duration` (default: 5s)
   - `pub max_delay: Duration` (default: 1800s / 30 min)
   - `pub poll_interval: Duration` (default: 500ms)
   - All use `std::time::Duration`. Default values from Architecture D1.2 / D2.3.
   - Add a `impl Default for WorkerConfig` that sets these plus the existing `concurrency: 4` and `log_payload: false`.

8. **Unit tests for `PostgresTaskRepository` claiming** in `crates/infrastructure/src/adapters/postgres_task_repository.rs` (inline `#[cfg(test)]` module) or `crates/infrastructure/tests/`:
   - **`claim_next_returns_running_task_with_correct_fields`** — insert a pending task, call `claim_next`, assert returned task has `status = Running`, `claimed_by` matches input `worker_id`, `claimed_until` is roughly `now() + lease_duration`, `attempts = 1`.
   - **`claim_next_returns_none_when_queue_empty`** — call `claim_next` on a queue with no pending tasks, assert `Ok(None)`.
   - **`claim_next_skips_future_scheduled_tasks`** — insert a task with `scheduled_at` 1 hour in the future, call `claim_next`, assert `Ok(None)`.
   - **`claim_next_respects_priority_ordering`** — insert 3 tasks with priorities 0, 5, 10; call `claim_next` 3 times, assert tasks returned in priority descending order (10, 5, 0).
   - **`claim_next_concurrent_no_duplicates`** (TEA P0-INT-002-007, **LOAD-BEARING TEST**) — insert 10 pending tasks, spawn 10 concurrent `claim_next` calls with different `worker_id`s, assert all 10 tasks claimed, each by exactly one worker (no duplicates). **Verify via raw `SELECT count(DISTINCT claimed_by) FROM tasks WHERE status = 'running'` returning 10.**
   - **`complete_transitions_to_completed`** — insert and claim a task, call `complete(id)`, assert returned task has `status = Completed`, `updated_at` refreshed.
   - **`complete_fails_for_non_running_task`** — insert a `Pending` task (not claimed), call `complete(id)`, assert error.
   - **`fail_retries_when_under_max_attempts`** — insert a task with `max_attempts = 3`, claim it (attempts becomes 1), call `fail(id, "oops")`, assert returned task has `status = Pending`, `claimed_by = None`, `claimed_until = None`, `last_error = "oops"`, `scheduled_at` is in the future (backoff applied).
   - **`fail_transitions_to_failed_when_max_attempts_reached`** — insert a task with `max_attempts = 1`, claim it (attempts becomes 1), call `fail(id, "fatal")`, assert returned task has `status = Failed`, `last_error = "fatal"`. **Verify via raw `SELECT status FROM tasks WHERE id = $1` that status is literally 'failed' in the database** (not just the return value).
   - **`fail_applies_exponential_backoff`** — insert a task with `max_attempts = 5`, claim it (attempts=1), fail it, claim again (attempts=2), fail again. Assert `scheduled_at` difference between first and second failure is roughly `base_delay * 2^(1-1) = 5s` vs `base_delay * 2^(2-1) = 10s`. Allow 2s tolerance for test timing.
   - **`fail_caps_backoff_at_max_delay`** — insert a task with `max_attempts = 20`, repeatedly claim+fail until `attempts` is high enough that `base_delay * 2^(attempts-1)` exceeds `max_delay`. Assert that `scheduled_at` offset is capped at `max_delay` (1800s).

9. **`#[instrument]` spans include `task_id`, `queue`, and `worker_id` fields** on all new methods per Architecture lines 692-702. Payload is NEVER in fields (FR38).

10. **Quality gates pass:**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace` — all existing 70 tests pass + new claiming tests.
    - `cargo deny check bans` — `bans ok`
    - `.sqlx/` offline cache regenerated after new queries added.

## Tasks / Subtasks

- [x] **Task 1: Add claiming config fields to `WorkerConfig`** (AC 7)
  - [x] Edit `crates/application/src/config.rs`: add `lease_duration`, `base_delay`, `max_delay`, `poll_interval` fields to `WorkerConfig` with `std::time::Duration` types.
  - [x] Add `impl Default for WorkerConfig` with values from Architecture D1.2/D2.3: `concurrency: 4`, `poll_interval: 500ms`, `lease_duration: 300s`, `base_delay: 5s`, `max_delay: 1800s`, `log_payload: false`.
  - [x] Run `cargo check -p iron-defer-application`.

- [x] **Task 2: Extend `TaskRepository` port with claiming methods** (AC 1)
  - [x] Edit `crates/application/src/ports/task_repository.rs`: add `claim_next`, `complete`, `fail` to the trait.
  - [x] Import `std::time::Duration`, `WorkerId` from domain, and `QueueName` from domain.
  - [x] The `#[automock]` annotation already covers new methods automatically.
  - [x] Run `cargo check -p iron-defer-application` — will fail because infra crate doesn't implement the new methods yet. That's expected.

- [x] **Task 3: Redesign `TaskExecutor` port** (AC 5)
  - [x] Edit `crates/application/src/ports/task_executor.rs`: change signature to `execute(&self, task: &TaskRecord, ctx: &TaskContext) -> Result<(), TaskError>`. Import `TaskContext` from domain.
  - [x] Update doc comment to reflect this is no longer a stub.

- [x] **Task 4: Add migration `0002_add_claim_check.sql`** (AC 6)
  - [x] Create `migrations/0002_add_claim_check.sql` with the `CHECK ((claimed_by IS NULL) = (claimed_until IS NULL))` constraint.
  - [x] Verify existing data complies: all existing rows have both NULL (newly created tasks have no claims).

- [x] **Task 5: Implement `PostgresTaskRepository::claim_next`** (AC 2)
  - [x] Add the `claim_next` method to the `impl TaskRepository for PostgresTaskRepository` block.
  - [x] Use `sqlx::query_as!` with the D2.1 atomic SKIP LOCKED query. Use `PgInterval` or seconds-to-interval SQL cast for `lease_duration`.
  - [x] Return `Ok(None)` on empty result set via `fetch_optional`.
  - [x] Add `#[instrument(skip(self), fields(queue = %queue, worker_id = %worker_id), err)]`.

- [x] **Task 6: Implement `PostgresTaskRepository::complete`** (AC 3)
  - [x] Add the `complete` method with the atomic UPDATE + status guard.
  - [x] Return error if no rows updated.
  - [x] Add `#[instrument(skip(self), fields(task_id = %task_id), err)]`.

- [x] **Task 7: Implement `PostgresTaskRepository::fail`** (AC 4)
  - [x] Add the `fail` method with the two-query retry/terminal pattern.
  - [x] Accept `base_delay` and `max_delay` as `f64` seconds parameters (or as `Duration` → convert to seconds for SQL).
  - [x] Use `truncate_last_error` on `error_message` before binding.
  - [x] Add `#[instrument(skip(self, error_message), fields(task_id = %task_id), err)]`.

- [x] **Task 8: Regenerate `.sqlx/` offline cache** (AC 10)
  - [x] Start a local Postgres container (or use the testcontainers approach).
  - [x] Run `cargo sqlx prepare --workspace` to regenerate the offline cache with the new queries.
  - [x] Verify `SQLX_OFFLINE=true cargo check --workspace` passes.

- [x] **Task 9: Integration tests for claiming** (AC 8)
  - [x] Add claiming integration tests in `crates/infrastructure/tests/` using the existing testcontainers pattern.
  - [x] Implement all 11 test cases from AC 8.
  - [x] The concurrent claiming test (`claim_next_concurrent_no_duplicates`) is the **SINGLE MOST LOAD-BEARING TEST** — it must verify via raw SQL that no duplicate claims exist, not just check return values.

- [x] **Task 10: Quality gates** (AC 10)
  - [x] `cargo fmt --check`
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
  - [x] `SQLX_OFFLINE=true cargo test --workspace`
  - [x] `cargo deny check bans`
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` returns empty.

## Dev Notes

### Architecture Compliance

- **D2.1 Atomic Claiming:** The SKIP LOCKED query is the correctness linchpin for the at-least-once guarantee. The exact SQL from Architecture §D2.1 lines 332–349 is normative. Do NOT restructure the query (e.g., separate SELECT + UPDATE) — atomicity depends on the single-statement pattern.
- **D1.2 Backoff Formula:** `now() + min(base_delay * 2^(attempts-1), max_delay)`. Compute in SQL using `make_interval(secs => LEAST($base_delay * power(2, attempts - 1), $max_delay))`. The `attempts` value is the POST-increment value (already +1 from the claim step).
- **D1.1 Schema:** No schema changes to the `tasks` table columns. AC 6 adds only a CHECK constraint. The existing `idx_tasks_claiming` partial index covers the claim subquery perfectly.

### Previous Story Intelligence (from Story 1A.3 & Epic 1A Retrospective)

**Code patterns established in 1A.1–1A.3 that MUST be followed:**
- `sqlx::query_as!` with `TaskRow` struct + `TryFrom<TaskRow> for TaskRecord` at the adapter boundary — validated in `crates/infrastructure/src/adapters/postgres_task_repository.rs:81-129`. Do NOT bypass this pattern for new queries.
- `truncate_last_error()` and `truncate_last_error_borrow()` in `crates/infrastructure/src/adapters/postgres_task_repository.rs:37-58` — truncate `last_error` to 4096 bytes on UTF-8 boundary. Use `truncate_last_error` on the write path in `fail()`.
- `PostgresAdapterError` → `TaskError::Storage` conversion via `From` impl in `crates/infrastructure/src/error.rs:33-42`. New adapter methods map `sqlx::Error` through `PostgresAdapterError::Query` automatically.
- `#[instrument(skip(self), fields(...), err)]` on every public async method. Payload NEVER in fields (FR38).
- Testcontainers `OnceCell<Option<TestDb>>` shared-pool pattern in `crates/infrastructure/tests/common/mod.rs`. Individual tests call `test_pool()`, never spin up their own container. Migrations run once in `get_or_init`.

**Key types and their locations:**
- `TaskRecord::new(14 args)` — `crates/domain/src/model/task.rs:64-136`
- `TaskId`, `WorkerId` — `crates/domain/src/model/task.rs:15-42`, `crates/domain/src/model/worker.rs:6-34`
- `QueueName` — `crates/domain/src/model/queue.rs:8-104`
- `TaskStatus` — `crates/domain/src/model/task.rs:50-62` (Pending, Running, Completed, Failed, Cancelled)
- `TaskError` — `crates/domain/src/error.rs:15-45` (AlreadyClaimed, InvalidPayload, ExecutionFailed, Storage)
- `ClaimError` — `crates/domain/src/error.rs:48-55` (NoneAvailable, Storage — currently unused, available if needed)
- `TaskRow` (internal) — `crates/infrastructure/src/adapters/postgres_task_repository.rs:60-79`
- `PostgresAdapterError` — `crates/infrastructure/src/error.rs:21-31`

**Retrospective action items applicable to THIS story:**
1. **Pre-implementation AC walkthrough:** read each AC line by line and explicitly mark verbatim/amended/disputed BEFORE writing code.
2. **`#[non_exhaustive]` types need constructors:** already done for `TaskRecord` and `TaskContext`.
3. **Call out the SINGLE most load-bearing test:** `claim_next_concurrent_no_duplicates` — verify via raw SQL, not just return values. See AC 8.
4. **Spec text is a contract, not a starting point.** Every deviation must be escalated as an explicit decision.

**Deferred work items relevant to this story (from `deferred-work.md`):**
- `TaskExecutor::execute` lacks `TaskContext` → RESOLVED by AC 5 of this story.
- `(claimed_by, claimed_until)` cross-field invariant → RESOLVED by AC 6 of this story.
- `attempts > max_attempts` cross-field invariant → RESOLVED by AC 4's terminal failure logic in `fail()`.
- `InvalidPayload` and `ExecutionFailed` remain stringly-typed → NOT in scope. Defer to Epic 1B.2 or later when the worker pool establishes concrete error shapes.

### Tooling Notes — Passing `Duration` as Postgres `INTERVAL`

sqlx does not have a built-in `Encode` impl for `std::time::Duration` → Postgres `INTERVAL`. Options:
1. **Use `sqlx::types::chrono::TimeDelta`** (alias for `chrono::Duration`) — has native Encode/Decode. Convert `std::time::Duration` → `chrono::TimeDelta::from_std(dur).unwrap()`.
2. **Use seconds as `f64`** and `make_interval(secs => $1)` in SQL — avoids any Rust-side interval type.
3. **Use `PgInterval` from `sqlx::postgres::types::PgInterval`** — the raw Postgres interval type. Construct from months/days/microseconds.

Recommended: option 2 (pass `lease_duration.as_secs_f64()` as `f64` and use `make_interval(secs => $1)` in SQL). This is the simplest, avoids adding `chrono` as a non-dev dependency to infrastructure (already there via workspace, but keeps the SQL transparent). If sqlx's `query_as!` macro complains about `f64` → `INTERVAL` type mismatch, cast in SQL: `$2::double precision` and wrap with `make_interval(secs => $2::double precision)`.

### Tooling Notes — `sqlx::query_as!` with `RETURNING *` after `UPDATE`

The `claim_next` query returns a `TaskRow` via `RETURNING *`. The `query_as!` macro handles this identically to a `SELECT` — the column names and types must match the `TaskRow` struct fields. The `UPDATE ... RETURNING *` pattern is already established by the `save()` method in Story 1A.2.

For `claim_next`, `fetch_optional` is correct — the subquery may match zero rows, returning zero rows from the UPDATE, which `fetch_optional` interprets as `None`.

### Tooling Notes — `fail()` Backoff Constants

The `fail()` method needs `base_delay` and `max_delay` to compute the backoff formula. Three design options:
1. **Pass as method parameters** — `fail(&self, task_id, error_message, base_delay_secs, max_delay_secs)`. Simple, explicit, but pollutes the port trait signature with config.
2. **Store on `PostgresTaskRepository`** — add config fields to the struct. Couples the adapter to config but keeps the trait clean.
3. **Hardcode in the SQL** — `LEAST(5 * power(2, attempts - 1), 1800)`. Simplest but not configurable.

Recommended: option 1 — pass `base_delay_secs: f64` and `max_delay_secs: f64` as parameters to the port trait's `fail()` method. The worker pool (Story 1B.2) will have access to `WorkerConfig` and can supply the values. This keeps the adapter stateless and the port trait configuration-agnostic. Update the trait signature accordingly:
```rust
async fn fail(
    &self,
    task_id: TaskId,
    error_message: &str,
    base_delay_secs: f64,
    max_delay_secs: f64,
) -> Result<TaskRecord, TaskError>;
```

### Critical Conventions (do NOT deviate)

- **`sqlx::query_as!` with `TaskRow` + `TryFrom<TaskRow>` for ALL database reads.** Never `query_scalar!`, never manual column mapping, never skip the `TryFrom` validation. The adapter boundary validates; the domain stays clean.
- **`truncate_last_error` on EVERY write path that touches `last_error`.** The `fail()` method must truncate before binding.
- **`#[instrument]` on every new async method.** `skip(self)` always. `skip(error_message)` in `fail()` to avoid logging potentially sensitive error content. Fields: `task_id`, `queue`, `worker_id` as appropriate. NEVER `payload`.
- **No `unwrap()` / `expect()` / `panic!()` in `src/`** outside `#[cfg(test)]`. Map errors to `TaskError::Storage` or `TaskError::InvalidPayload`.
- **`TaskRegistry::new()` is constructed ONLY in `crates/api/src/lib.rs`.** This story does not touch the registry.
- **Builder NEVER spawns a Tokio runtime.** Not relevant to this story (no builder changes) but remains a hard invariant.
- **Error source chains preserved.** All `sqlx::Error` flows through `PostgresAdapterError::Query { #[from] source: sqlx::Error }` → `TaskError::Storage { source: Box<dyn Error> }`. Never discard error context.

### Out of Scope for This Story

- **Worker pool / poll loop / `JoinSet` / `Semaphore`** — Story 1B.2.
- **REST API** — Story 1B.3.
- **Sweeper / zombie recovery** — Story 2.1 (uses the `idx_tasks_zombie` index).
- **Graceful shutdown / `CancellationToken`** — Story 2.2.
- **OTel metrics** — Epic 3 (but `#[instrument]` tracing spans are in scope).
- **`IronDefer::start()` method** — Story 1B.2.
- **`IronDeferBuilder` new methods (`.concurrency()`, `.poll_interval()`, etc.)** — Story 1B.2.
- **Integration with `TaskHandlerAdapter::execute`** — Story 1B.2 (the worker pool dispatches through the registry → adapter → `Task::execute`).
- **`ClaimError` usage** — the domain crate defines `ClaimError` but this story uses `TaskError` for the port trait methods. If the dev finds `ClaimError` more appropriate for `claim_next`, escalate as a decision rather than silently switching.

### Project Structure Notes

- **Modified files:**
  - `crates/application/src/config.rs` — `WorkerConfig` gains fields + `Default` impl
  - `crates/application/src/ports/task_repository.rs` — 3 new trait methods
  - `crates/application/src/ports/task_executor.rs` — signature redesign
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — 3 new method impls + tests
- **New files:**
  - `migrations/0002_add_claim_check.sql`
- **Regenerated files:**
  - `.sqlx/` offline cache (new query hashes)

### References

- [Source: `docs/artifacts/planning/architecture.md` §D2.1 lines 328-353] — Atomic SKIP LOCKED claiming query
- [Source: `docs/artifacts/planning/architecture.md` §D1.2 lines 307-318] — Retry/backoff formula and defaults
- [Source: `docs/artifacts/planning/architecture.md` §D2.2 lines 355-362] — Worker pool concurrency model (Epic 1B.2 context)
- [Source: `docs/artifacts/planning/architecture.md` lines 692-702] — `#[instrument]` conventions
- [Source: `docs/artifacts/planning/epics.md` lines 370-408] — Story 1B.1 acceptance criteria
- [Source: `docs/artifacts/implementation/epic-1a-retro-2026-04-06.md`] — Retrospective lessons and action items
- [Source: `docs/artifacts/implementation/deferred-work.md`] — Deferred items resolved by this story
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs`] — Existing adapter patterns
- [Source: `crates/application/src/ports/task_executor.rs`] — Current stub to be redesigned
- [Source: `crates/application/src/config.rs`] — WorkerConfig struct

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None — clean implementation with no blockers or failures.

### Completion Notes List

- **Task 1:** Added `lease_duration` (300s), `base_delay` (5s), `max_delay` (1800s), `poll_interval` (500ms) to `WorkerConfig` with custom `Default` impl. Duration fields use `#[serde(skip)]` since the figment config loading chain is not yet implemented — proper serde support deferred to the config story. `concurrency` default changed from 0 to 4 per Architecture D2.3.
- **Task 2:** Added `claim_next`, `complete`, `fail` to `TaskRepository` trait. `fail` takes `base_delay_secs`/`max_delay_secs` as `f64` parameters per Dev Notes option 1 (keeps adapter stateless, worker pool in 1B.2 supplies from `WorkerConfig`).
- **Task 3:** Redesigned `TaskExecutor::execute` to accept `(&TaskRecord, &TaskContext)`. Resolves deferred-work item.
- **Task 4:** Created migration `0002_add_claim_check.sql` with `CHECK ((claimed_by IS NULL) = (claimed_until IS NULL))`. Resolves deferred-work item.
- **Task 5:** Implemented `claim_next` with D2.1 atomic SKIP LOCKED query. Uses `make_interval(secs => $2)` for lease duration (Dev Notes option 2). `fetch_optional` returns `Ok(None)` on empty queue.
- **Task 6:** Implemented `complete` with atomic UPDATE guarded by `AND status = 'running'`. Returns `TaskError::InvalidPayload` on miss.
- **Task 7:** Implemented `fail` with two-query retry/terminal pattern. `truncate_last_error` applied before binding. Backoff formula computed in SQL: `LEAST($3 * power(2, attempts - 1), $4)`.
- **Task 8:** Regenerated `.sqlx/` offline cache with 4 new query hashes (claim_next, complete, fail-retry, fail-terminal).
- **Task 9:** All 11 integration tests implemented and passing. `claim_next_concurrent_no_duplicates` (LOAD-BEARING) verifies via raw SQL `SELECT count(DISTINCT claimed_by)` that 10 concurrent workers each claim exactly 1 of 10 tasks.
- **Task 10:** All quality gates pass: `fmt`, `clippy --pedantic`, 82 tests, `cargo deny`, no openssl/native-tls.

### File List

- `crates/application/src/config.rs` — modified (WorkerConfig fields + Default impl)
- `crates/application/src/ports/task_repository.rs` — modified (3 new trait methods)
- `crates/application/src/ports/task_executor.rs` — modified (signature redesign)
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — modified (3 new method impls)
- `crates/infrastructure/tests/task_repository_test.rs` — modified (11 new integration tests)
- `migrations/0002_add_claim_check.sql` — new
- `.sqlx/query-*.json` — regenerated (4 new query hashes)

### Senior Developer Review (AI)

**Review Date:** 2026-04-09
**Review Outcome:** Approve
**Reviewer Model:** Claude Opus 4.6 (1M context) — 3 parallel review layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor)

**Summary:** 0 decision-needed, 0 patch, 3 defer, 15 dismissed.

All 10 Acceptance Criteria substantively implemented. No blocking issues. Three deferred items are pre-existing design decisions or documentation gaps, not code defects.

**Action Items:**
- [x] [Review][Defer] `#[serde(skip)]` on Duration fields makes them unconfigurable via config files — intentional; config loading chain story will add proper Duration serde support.
- [x] [Review][Defer] `fail()` port trait signature (4 params) is wider than AC 1 spec text (2 params) — Dev Notes option 1 sanctions this; amend AC 1 text to match actual signature.
- [x] [Review][Defer] `fail()` f64 parameters (`base_delay_secs`, `max_delay_secs`) lack NaN/negative/zero input validation — all real callers use `WorkerConfig` Duration which is always non-negative; add validation when worker pool (1B.2) lands.

### Change Log

- 2026-04-09: Implemented Story 1B.1 — atomic claiming (SKIP LOCKED), task completion, retry/terminal failure with exponential backoff, CHECK constraint for claim fields, TaskExecutor redesign, 11 integration tests. All 82 tests pass, all quality gates green.
- 2026-04-09: Code review (3 layers) — Approved. 0 patches, 3 deferred items (serde configurability, AC 1 text amendment, f64 validation), 15 dismissed as spec-compliant or false positives.
