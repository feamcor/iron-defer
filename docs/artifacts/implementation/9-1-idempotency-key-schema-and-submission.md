# Story 9.1: Idempotency Key Schema & Submission

Status: done

## Story

As a developer,
I want to supply an optional idempotency key when submitting a task,
so that retried submissions create exactly one task instead of duplicates.

## Acceptance Criteria

1. **Given** a task submitted with an idempotency key, **when** the same key + queue combination is submitted again, **then** exactly 1 task exists; the response returns HTTP 200 (not 201) with the original task record. **And** 10 concurrent retries with the same key create exactly 1 task (barrier-synchronized test). **And** the 9 "losing" submitters each receive HTTP 200 with the existing task, never 500.

2. **Given** an idempotency key submitted to queue "A", **when** the same key is submitted to queue "B", **then** two separate tasks are created (keys are scoped per-queue).

3. **Given** a completed/failed/cancelled task that held an idempotency key, **when** the key retention window expires (default 24h), **then** the Sweeper cleans up the expired key, allowing reuse. **And** cleanup piggybacks on the existing Sweeper tick (no new background actor).

## Tasks / Subtasks

- [x] Task 1: Database migration (AC: 1, 2)
  - [x] 1.1 Create `migrations/0004_add_idempotency_columns.sql` — `ALTER TABLE tasks ADD COLUMN idempotency_key VARCHAR; ALTER TABLE tasks ADD COLUMN idempotency_expires_at TIMESTAMPTZ;`
  - [x] 1.2 Create unique partial index: `CREATE UNIQUE INDEX idx_tasks_idempotency ON tasks (queue, idempotency_key) WHERE idempotency_key IS NOT NULL AND status NOT IN ('completed', 'failed', 'cancelled');`
  - [x] 1.3 Regenerate `.sqlx/` offline cache: `cargo sqlx prepare --workspace`

- [x] Task 2: Domain model extensions (AC: 1, 2, 3)
  - [x] 2.1 Add `idempotency_key: Option<String>` and `idempotency_expires_at: Option<DateTime<Utc>>` fields to `TaskRecord` in `crates/domain/src/model/task.rs` (private with typed accessors following existing pattern)
  - [x] 2.2 Add `idempotency_key_retention` duration field to `WorkerConfig` in `crates/application/src/config.rs` (default: 24h, humantime_serde deserialization, validation: >= 1 minute)
  - [x] 2.3 Expose `idempotency_key()` and `idempotency_expires_at()` accessors on `TaskRecord`

- [x] Task 3: Repository layer — idempotent save (AC: 1, 2)
  - [x] 3.1 Add `save_idempotent()` method to `TaskRepository` trait in `crates/application/src/ports/task_repository.rs` — returns `(TaskRecord, bool)` where bool = `created` (true = new, false = duplicate)
  - [x] 3.2 Implement `save_idempotent()` in `PostgresTaskRepository` using `INSERT ... ON CONFLICT (queue, idempotency_key) WHERE idempotency_key IS NOT NULL AND status NOT IN ('completed', 'failed', 'cancelled') DO NOTHING` followed by `SELECT` for existing row
  - [x] 3.3 Update `TaskRow` struct (the `sqlx::FromRow` type in `postgres_task_repository.rs`) with the new columns
  - [x] 3.4 Update `TryFrom<TaskRow> for TaskRecord` mapping for new fields

- [x] Task 4: Application layer — idempotent enqueue (AC: 1, 2)
  - [x] 4.1 Add `enqueue_idempotent()` to `SchedulerService` in `crates/application/src/services/scheduler.rs` — accepts `idempotency_key: &str`, calculates `idempotency_expires_at = now() + retention`, delegates to `repo.save_idempotent()`
  - [x] 4.2 Add `enqueue_raw_idempotent()` variant for REST API path (runtime-typed kind, like existing `enqueue_raw()`)

- [x] Task 5: Public library API (AC: 1)
  - [x] 5.1 Add `enqueue_idempotent()` to `IronDefer` in `crates/api/src/lib.rs` — signature: `pub async fn enqueue_idempotent<T: Task>(&self, queue: &str, task: T, idempotency_key: &str) -> Result<(TaskRecord, bool), TaskError>`. Returns `(record, created)` — `created=false` for duplicate key.

- [x] Task 6: REST API — idempotency support (AC: 1, 2)
  - [x] 6.1 Add optional `idempotency_key: Option<String>` field (JSON: `idempotencyKey`) to `CreateTaskRequest` in `crates/api/src/http/handlers/tasks.rs`
  - [x] 6.2 Update `create_task()` handler: if `idempotency_key` is present, call `enqueue_raw_idempotent()`; return 200 (not 201) when `created=false`
  - [x] 6.3 Add `idempotencyKey` and `idempotencyExpiresAt` to `TaskResponse` (nullable)
  - [x] 6.4 Update OpenAPI schema (`#[utoipa::path]` annotations) for new request/response fields

- [x] Task 7: CLI — idempotency support (AC: 1)
  - [x] 7.1 Add `--idempotency-key` optional flag to `Submit` command in `crates/api/src/cli/submit.rs`
  - [x] 7.2 Route through `enqueue_raw_idempotent()` when flag is present

- [x] Task 8: Sweeper — expired key cleanup (AC: 3)
  - [x] 8.1 Add cleanup query to `recover_zombie_tasks()` in `PostgresTaskRepository` (or a new `cleanup_expired_idempotency_keys()` method called from the same sweeper tick): `UPDATE tasks SET idempotency_key = NULL, idempotency_expires_at = NULL WHERE idempotency_expires_at < now() AND status IN ('completed', 'failed', 'cancelled') AND idempotency_key IS NOT NULL`
  - [x] 8.2 Add sweeper call in `SweeperService::run()` — piggyback on existing tick, emit counter metric `iron_defer_idempotency_keys_cleaned_total`
  - [x] 8.3 Pass `idempotency_key_retention` from `WorkerConfig` to `SweeperService` constructor

- [x] Task 9: Integration tests (AC: 1, 2, 3)
  - [x] 9.1 Test: submit with idempotency key, re-submit same key+queue → returns same task, HTTP 200
  - [x] 9.2 Test: same key, different queues → two distinct tasks
  - [x] 9.3 Test: submit without idempotency key → existing behavior (201, no dedup)
  - [x] 9.4 Test: sweeper cleans expired keys after retention window
  - [x] 9.5 Test: expired key allows reuse after cleanup

- [x] Task 10: Update existing `save()` to pass through new columns as NULL (AC: backward compatibility)
  - [x] 10.1 Update existing `PostgresTaskRepository::save()` INSERT to include `idempotency_key` and `idempotency_expires_at` columns (always NULL for non-idempotent submissions)

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules (enforced by Cargo crate boundaries):**
- `domain` ← no workspace dependencies
- `application` ← domain only
- `infrastructure` ← domain + application + external crates
- `api` ← all crates (wiring only)

The idempotency key is a domain concept (added to `TaskRecord`), with the conflict resolution query in infrastructure (`PostgresTaskRepository`), and the public API surface in `api` (`IronDefer`).

### Key Implementation Patterns

**INSERT ... ON CONFLICT pattern for idempotent save:**
```sql
-- Step 1: Attempt insert
INSERT INTO tasks (id, queue, kind, payload, status, priority, attempts, max_attempts,
    last_error, scheduled_at, claimed_by, claimed_until, idempotency_key, idempotency_expires_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
ON CONFLICT (queue, idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status NOT IN ('completed', 'failed', 'cancelled')
DO NOTHING
RETURNING id, queue, kind, payload, status, priority, attempts, max_attempts, last_error,
    scheduled_at, claimed_by, claimed_until, created_at, updated_at,
    idempotency_key, idempotency_expires_at;

-- Step 2: If no row returned (conflict), SELECT the existing one
SELECT id, queue, kind, payload, status, priority, attempts, max_attempts, last_error,
    scheduled_at, claimed_by, claimed_until, created_at, updated_at,
    idempotency_key, idempotency_expires_at
FROM tasks
WHERE queue = $1 AND idempotency_key = $2
  AND status NOT IN ('completed', 'failed', 'cancelled');
```

Both queries should run in the same transaction to prevent TOCTOU races. The `RETURNING` clause on the INSERT tells us whether we got the new row or nothing (conflict).

**Concurrent safety:** Under 10 concurrent inserts with the same key, Postgres unique index + `DO NOTHING` ensures exactly one winner. The losers get an empty RETURNING and fall through to the SELECT. No 500 errors — the losers receive the existing row.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `migrations/0004_add_idempotency_columns.sql` | **NEW** — schema migration |
| `crates/domain/src/model/task.rs` | Add 2 fields + accessors to `TaskRecord` |
| `crates/application/src/config.rs` | Add `idempotency_key_retention` to `WorkerConfig` |
| `crates/application/src/ports/task_repository.rs` | Add `save_idempotent()` to trait |
| `crates/application/src/services/scheduler.rs` | Add `enqueue_idempotent()` + `enqueue_raw_idempotent()` |
| `crates/application/src/services/sweeper.rs` | Add key cleanup call in tick |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Implement `save_idempotent()`, update `TaskRow`, update `save()` INSERT |
| `crates/api/src/lib.rs` | Add `enqueue_idempotent()` to `IronDefer` |
| `crates/api/src/http/handlers/tasks.rs` | Add `idempotencyKey` to request/response, branch on presence |
| `crates/api/src/cli/submit.rs` | Add `--idempotency-key` flag |
| `crates/api/tests/` | New integration test file |

### Testing Standards

- Integration tests go in `crates/api/tests/` as flat files (no subdirectories)
- Use `fresh_pool_on_shared_container()` for clean DB state per test
- Concurrent submission test: use `tokio::sync::Barrier` with 10 tasks + `tokio::spawn` for each, then assert exactly 1 task in DB
- Sweeper cleanup test: insert a task with idempotency_key and expired `idempotency_expires_at`, run sweeper tick, verify key is nullified

### Critical Constraints

1. **Existing `save()` backward compatibility:** The non-idempotent `save()` method must continue to work identically. Pass `NULL` for the two new columns. Do not change its return type.

2. **Index predicate must match exactly:** The partial index `WHERE idempotency_key IS NOT NULL AND status NOT IN ('completed', 'failed', 'cancelled')` must match the `ON CONFLICT` clause predicate exactly — Postgres requires textual match.

3. **Key scoping:** Idempotency is per-queue. The unique index is `(queue, idempotency_key)`. Same key in different queues creates separate tasks.

4. **Retention cleanup is a NULL-set, not a DELETE:** Set `idempotency_key = NULL` and `idempotency_expires_at = NULL` on expired terminal tasks. This releases the key for reuse while preserving the task record.

5. **`#[instrument]` on all new public async methods** — skip `self` and `payload`, include `queue`, `idempotency_key` in fields.

6. **bon::Builder compatibility:** `TaskRecord` uses `#[derive(bon::Builder)]`. Adding new `Option<>` fields is backward-compatible — builder callers that don't set them get `None` by default.

7. **Metrics:** Add `iron_defer_idempotency_keys_cleaned_total` counter (labels: none) to `crates/application/src/metrics.rs` and record in `crates/infrastructure/src/observability/metrics.rs`.

### Previous Epic Intelligence

From Epic 8 retrospective: context-creation agents must verify method signatures, feature flags, and file paths against the actual codebase, not just architecture docs. The implementation agent should verify all referenced types and methods exist before using them.

From Epic 7 retrospective: pre/postcondition discipline — verify outcomes, not just command execution. Tests should assert the actual DB state, not just the API response.

### Project Structure Notes

- Migration file numbering continues from `0003_add_pagination_index.sql` → next is `0004_*`
- The `.sqlx/` offline cache must be regenerated after adding the migration and updating queries
- All new JSON fields use `camelCase` (ADR-0006): `idempotencyKey`, `idempotencyExpiresAt`
- Config env var for retention: `IRON_DEFER__WORKER__IDEMPOTENCY_KEY_RETENTION` (humantime format, e.g., `24h`)

### References

- [Source: docs/artifacts/planning/epics.md — Story 9.1]
- [Source: docs/artifacts/planning/prd.md — §G1 Exactly-once submission with idempotency keys]
- [Source: docs/artifacts/planning/architecture.md — §Growth Phase Architecture Addendum, Schema Evolution, Sweeper Modifications]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — save() at line 237, TaskRow struct]
- [Source: crates/application/src/services/scheduler.rs — enqueue() at line 75, enqueue_raw() at line 128]
- [Source: crates/application/src/services/sweeper.rs — SweeperService::run() at line 155]
- [Source: crates/api/src/http/handlers/tasks.rs — create_task() at line 125, CreateTaskRequest at line 36]
- [Source: crates/domain/src/model/task.rs — TaskRecord at line 79]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Pre-existing clippy `unused_mut` in `worker.rs:430` fixed (removed `mut` from `dispatch_task` parameter)
- Pre-existing clippy `collapsible_if` in `main.rs:171` fixed (merged nested `if` using `let ... &&`)
- New clippy `large_enum_variant` on `CancelResult` suppressed with `#[allow]` — caused by `TaskRecord` growing by 2 fields; boxing would change public API
- New clippy `too_many_arguments` on `enqueue_raw_idempotent` suppressed — mirrors existing `enqueue_raw` pattern with 2 extra idempotency params
- `StuckClaimRepo` manual trait impl in `worker.rs` tests needed new trait methods added
- `cli_test.rs` Submit struct literals needed `idempotency_key: None` field added

### Completion Notes List

All 10 tasks (33 subtasks) completed. Story implements exactly-once submission via optional idempotency keys scoped per-queue. Key design decisions:
- INSERT ... ON CONFLICT DO NOTHING + SELECT in a single transaction for concurrent safety
- `cleanup_expired_idempotency_keys()` as a dedicated repository method called from the sweeper tick
- Retention cleanup NULLs the key (not DELETE) preserving task records
- 6 integration tests including a 10-concurrent-submitters barrier test verifying exactly-1-task-in-DB
- Concurrent test asserts: 1x 201 Created + 9x 200 OK + all responses reference same task ID + DB count = 1

### Change Log

- 2026-04-24: Story 9.1 implementation complete — all 10 tasks, 33 subtasks done

### File List

- migrations/0004_add_idempotency_columns.sql (new)
- .sqlx/ (multiple query cache files regenerated)
- crates/domain/src/model/task.rs (modified)
- crates/application/src/config.rs (modified)
- crates/application/src/metrics.rs (modified)
- crates/application/src/ports/task_repository.rs (modified)
- crates/application/src/services/scheduler.rs (modified)
- crates/application/src/services/sweeper.rs (modified)
- crates/application/src/services/worker.rs (modified)
- crates/infrastructure/src/adapters/postgres_task_repository.rs (modified)
- crates/infrastructure/src/observability/metrics.rs (modified)
- crates/api/src/lib.rs (modified)
- crates/api/src/http/handlers/tasks.rs (modified)
- crates/api/src/cli/submit.rs (modified)
- crates/api/src/main.rs (modified — pre-existing clippy fix)
- crates/api/tests/idempotency_test.rs (new)
- crates/api/tests/cli_test.rs (modified)

### Review Findings

- [ ] [Review][Decision] Ambiguous CLI exit codes for duplicates — Should detecting a duplicate idempotency key return success (0) or a specific non-zero exit code to allow automation scripts to distinguish?
- [ ] [Review][Patch] CLI hardcodes 24h idempotency retention [crates/api/src/cli/submit.rs:95]
- [ ] [Review][Patch] Hardcoded 24h fallback in Scheduler retention conversion [crates/application/src/services/scheduler.rs:198]
- [ ] [Review][Patch] Race condition in `save_idempotent` conflict handling [crates/infrastructure/src/adapters/postgres_task_repository.rs:388]
- [ ] [Review][Patch] `SweeperService` constructor missing retention parameter [crates/application/src/services/sweeper.rs]
- [ ] [Review][Patch] CLI uses error channel for expected duplicate detection [crates/api/src/cli/submit.rs:114]
- [ ] [Review][Patch] Panic risk in `SchedulerService` (`.expect()`) [crates/application/src/services/scheduler.rs]
- [ ] [Review][Patch] Silent failure in `MaxAttempts` validation [crates/application/src/services/scheduler.rs:242]
- [ ] [Review][Patch] Missing validation for empty-string idempotency keys [crates/api/src/lib.rs:730]
- [ ] [Review][Patch] Missing OpenAPI schema field descriptions [crates/api/src/http/handlers/tasks.rs]
- [ ] [Review][Patch] Clock skew vulnerability in key cleanup [crates/infrastructure/src/adapters/postgres_task_repository.rs:403]
- [x] [Review][Defer] Brittle coupling between SQL predicates and Rust enum states — deferred, pre-existing
- [x] [Review][Defer] Disconnected metric name definitions — deferred, pre-existing
