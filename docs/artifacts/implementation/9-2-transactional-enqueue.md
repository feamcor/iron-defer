# Story 9.2: Transactional Enqueue

Status: done

## Story

As a developer using iron-defer as an embedded library,
I want to enqueue a task inside my application's database transaction,
so that the task and my business event are atomic — rollback erases both.

## Acceptance Criteria

1. **Given** a caller-provided `sqlx::Transaction`, **when** `engine.enqueue_in_tx(&mut tx, queue, task)` is called and the transaction commits, **then** the task becomes visible to workers.

2. **Given** a caller-provided `sqlx::Transaction`, **when** `engine.enqueue_in_tx(&mut tx, queue, task)` is called and the transaction rolls back, **then** zero tasks are visible in the queue, and zero worker activations occur during the rollback window.

3. **Given** transactional enqueue with idempotency key, **when** duplicate key detection occurs inside the caller's transaction, **then** deduplication is scoped to the transaction (uncommitted rows are invisible to other transactions via Postgres MVCC READ COMMITTED).

## Tasks / Subtasks

- [x] Task 1: Repository layer — transactional save (AC: 1, 2)
  - [x] 1.1 Add `save_in_tx()` method to `TaskRepository` trait in `crates/application/src/ports/task_repository.rs` — signature: `async fn save_in_tx(&self, tx: &mut sqlx::Transaction<'_, Postgres>, record: &TaskRecord) -> Result<TaskRecord, TaskError>`
  - [x] 1.2 Implement `save_in_tx()` in `PostgresTaskRepository` — single `INSERT INTO tasks ... RETURNING *` executing on `&mut *tx` (the transaction connection), NOT on `self.pool`
  - [x] 1.3 Add `save_idempotent_in_tx()` for the G1+G2 interaction — same `ON CONFLICT DO NOTHING` + `SELECT` pattern from Story 9.1, but executing on the caller's transaction

- [x] Task 2: Application layer — transactional enqueue (AC: 1, 2)
  - [x] 2.1 Add `enqueue_in_tx()` to `SchedulerService` — accepts `&mut sqlx::Transaction<'_, Postgres>`, constructs `TaskRecord`, calls `repo.save_in_tx(tx, &record)`
  - [x] 2.2 Add `enqueue_in_tx_idempotent()` variant for G1+G2 interaction — accepts idempotency_key in addition to transaction

- [x] Task 3: Public library API (AC: 1, 2, 3)
  - [x] 3.1 Add `enqueue_in_tx()` to `IronDefer` in `crates/api/src/lib.rs` — signature: `pub async fn enqueue_in_tx<'a, T: Task>(&self, tx: &mut sqlx::Transaction<'a, Postgres>, queue: &str, task: T) -> Result<TaskRecord, TaskError>`
  - [x] 3.2 Add `enqueue_in_tx_idempotent()` — combines transactional enqueue with idempotency key

- [x] Task 4: Integration tests (AC: 1, 2, 3)
  - [x] 4.1 Test: enqueue in committed transaction → task visible to workers, eventually reaches `completed`
  - [x] 4.2 Test: enqueue in rolled-back transaction → zero tasks in DB after rollback, zero tasks ever claimed by workers
  - [x] 4.3 Test: enqueue with idempotency key inside transaction, same key in separate transaction → deduplication works correctly (MVCC isolation)
  - [x] 4.4 Test: enqueue inside transaction, concurrent worker polls during uncommitted window → worker sees zero tasks (SKIP LOCKED + READ COMMITTED skips uncommitted rows)

- [x] Task 5: Documentation (AC: 1, 2)
  - [x] 5.1 Add inline doc comments on `enqueue_in_tx()` explaining: caller owns the transaction, caller must commit/rollback, iron-defer does not hold its own transaction open

## Dev Notes

### Architecture Compliance

**This is an embedded-library-only feature.** The REST API does NOT support transactional enqueue — it would require exposing transaction handles across HTTP, which is architecturally impossible. The CLI also does not support it (no persistent transaction context).

**Layer placement:**
- `TaskRepository` trait gains `save_in_tx()` — the port accepts a generic transaction reference
- `PostgresTaskRepository` implements it using `&mut *tx` as the executor
- `SchedulerService` gains `enqueue_in_tx()` — same validation as `enqueue()`, different persistence call
- `IronDefer` (api crate) exposes the public method

**`sqlx::Transaction` crosses the public API boundary.** This is a NEW boundary crossing that requires updating the architecture docs. Currently, `crates/api/src/lib.rs` lines 44-45 explicitly state only `PgPool` and `Migrator` may cross the boundary. `sqlx::Transaction<'_, Postgres>` becomes the third allowed sqlx type. Update the architecture comment in `lib.rs` to document this exception. Both `PgPool` and `Transaction` are caller-provided — iron-defer never constructs them.

**Task 0 (prerequisite):** Update the architecture boundary comment in `crates/api/src/lib.rs` (around line 44) to allow `sqlx::Transaction` as a permitted public API type.

### Key Implementation Patterns

**Single INSERT on caller's transaction — NO extra queries:**
```rust
pub async fn save_in_tx(
    &self,
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record: &TaskRecord,
) -> Result<TaskRecord, TaskError> {
    let row = sqlx::query_as!(
        TaskRow,
        r#"INSERT INTO tasks (id, queue, kind, payload, status, priority,
            attempts, max_attempts, last_error, scheduled_at, claimed_by, claimed_until,
            idempotency_key, idempotency_expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        RETURNING *"#,
        // ... bind params
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(/* PostgresAdapterError conversion */)?;
    TaskRecord::try_from(row).map_err(Into::into)
}
```

**Critical constraints from PRD:**
- The engine must NOT acquire additional locks or issue additional queries beyond the single INSERT
- The engine must NOT hold its own transaction open during the caller's transaction
- No `BEGIN`/`COMMIT` — the caller's transaction is already open

**MVCC guarantees (READ COMMITTED):**
- `SKIP LOCKED` naturally skips uncommitted rows at READ COMMITTED isolation
- Workers polling during the caller's uncommitted window see zero new tasks
- After commit, the next poll cycle picks up the task normally
- After rollback, the row never existed (MVCC undo)

### Idempotent Transactional Enqueue (G1+G2 Interaction)

When both features are used together:
```rust
engine.enqueue_in_tx_idempotent(&mut tx, "payments", task, "payment-123").await?;
```

The `ON CONFLICT DO NOTHING` + `SELECT` pattern from Story 9.1 executes on the caller's transaction. Duplicate detection is transaction-scoped — uncommitted rows from other transactions are invisible, so two concurrent transactions with the same key will both succeed at INSERT. The unique index resolves the conflict at commit time (one commits, the other gets a serialization error if using SERIALIZABLE, or succeeds with a duplicate at READ COMMITTED — but the partial index prevents the duplicate from violating uniqueness for active tasks).

**Important:** At READ COMMITTED (Postgres default), two concurrent transactions can both INSERT the same idempotency key if neither has committed yet. The unique partial index prevents both from being visible simultaneously — one INSERT will block until the other commits or rolls back, then either succeeds (if the other rolled back) or gets a conflict (if the other committed). This is correct behavior.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `crates/application/Cargo.toml` | **ADD** `sqlx = { workspace = true, default-features = false, features = ["postgres"] }` — required for `sqlx::Transaction` type |
| `crates/application/src/ports/task_repository.rs` | Add `save_in_tx()` + `save_idempotent_in_tx()` to trait (or a separate `TransactionalTaskRepository` trait) |
| `crates/application/src/services/scheduler.rs` | Add `enqueue_in_tx()` + `enqueue_in_tx_idempotent()` |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Implement `save_in_tx()` + `save_idempotent_in_tx()` |
| `crates/api/src/lib.rs` | Update architecture boundary comment; add `enqueue_in_tx()` + `enqueue_in_tx_idempotent()` to `IronDefer` |
| `crates/api/tests/` | New integration test file |

### Testing Standards

- Integration tests go in `crates/api/tests/` as flat files
- Use `fresh_pool_on_shared_container()` for clean DB state
- **Rollback test pattern:**
  1. Begin transaction on the test pool
  2. Call `engine.enqueue_in_tx(&mut tx, ...)` 
  3. `tx.rollback().await`
  4. Query `SELECT count(*) FROM tasks WHERE queue = $1` on the pool — assert 0
  5. Sleep briefly + re-query to confirm workers didn't claim anything during the window

- **Commit test pattern:**
  1. Boot the E2E engine with workers
  2. Begin transaction on the pool
  3. Enqueue in tx
  4. Assert zero tasks visible before commit (query from a separate connection)
  5. Commit
  6. Wait for task to reach `completed` status via `wait_for_status()`

- **MVCC isolation test:** Use two separate transactions. Enqueue same idempotency key in both. Commit one, verify the other gets conflict or succeeds based on timing.

### Critical Constraints

1. **NO schema changes required.** Transactional enqueue reuses the existing `tasks` table and the idempotency columns from Story 9.1. No new migration.

2. **`sqlx::Transaction` import in the application crate — NEW DEPENDENCY REQUIRED.** The `application` crate currently has NO sqlx dependency. Adding `save_in_tx()` to `TaskRepository` requires adding sqlx to `crates/application/Cargo.toml`: `sqlx = { workspace = true, default-features = false, features = ["postgres"] }`. This is the minimum required for the `sqlx::Transaction<'_, Postgres>` and `sqlx::Postgres` types. Alternatively, create a separate `TransactionalTaskRepository` trait in the infrastructure crate to avoid this dependency — but the PRD specifies the method on the public API, so the sqlx type must cross layers. Add this dependency explicitly.

3. **mockall compatibility — CONFIRMED PROBLEMATIC.** The `save_in_tx()` method on `TaskRepository` takes `&mut sqlx::Transaction<'_, Postgres>` — mockall 0.13 has known issues with mutable references to generic types with lifetime parameters. `#[automock]` will likely fail to compile for these methods. **Recommended approach:** (a) create a separate `TransactionalTaskRepository` trait WITHOUT `#[automock]` for the transaction methods, keeping the base `TaskRepository` mockable, OR (b) skip unit tests for transaction methods entirely and rely on integration tests (the existing pattern in `recover_zombie_tasks()` uses transactions without mocking). Choose option (b) as the simpler path — transaction methods are inherently infrastructure-bound and testing via mocks adds no value.

4. **`#[instrument]` on all new public async methods** — skip `self`, `tx`, and `payload`. Include `queue` in fields.

5. **NFR-R8:** Transactional enqueue must not extend the caller's transaction duration by more than 10ms at p99. This means: single INSERT, no advisory locks, no extra SELECTs (except for idempotent variant's conflict resolution SELECT).

### Previous Story Intelligence

**Story 9.1 dependencies:** This story builds on 9.1's schema (idempotency columns must exist). The `save_in_tx()` INSERT must include the idempotency columns (as NULL for non-idempotent path).

**From Epic 8 retrospective:** Verify method signatures against the actual codebase before referencing them. The `PostgresTaskRepository` uses `sqlx::query_as!` macro with `.fetch_one(&self.pool)` — the transactional variant must use `.fetch_one(&mut *tx)` instead (single deref, matching the pattern in `recover_zombie_tasks()` at line 514 which uses `.fetch_all(&mut *tx)`).

### Project Structure Notes

- No new files created except the integration test
- The `sqlx::Transaction` type requires `sqlx` with `postgres` feature in `crates/application/Cargo.toml`
- The `Postgres` type parameter comes from `sqlx::Postgres`

### References

- [Source: docs/artifacts/planning/epics.md — Story 9.2]
- [Source: docs/artifacts/planning/prd.md — §G2 Transactional enqueue (River pattern)]
- [Source: docs/artifacts/planning/architecture.md — §Growth Phase Architecture Addendum, New API Surface, Cross-Feature Interaction Matrix]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — save() at line 237]
- [Source: crates/application/src/services/scheduler.rs — enqueue() at line 75]
- [Source: crates/application/src/ports/task_repository.rs — TaskRepository trait]
- [Source: crates/api/src/lib.rs — IronDefer public API]

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### Debug Log References
- Pre-existing SweeperService::new 4-arg mismatch from Story 9.1 — fixed in api/src/lib.rs and sweeper_test.rs
- Stale sqlx offline cache from Story 9.1 — regenerated .sqlx/ cache
- bon builder cache invalidation triggered by adding sqlx dep to application crate — resolved via clean build

### Completion Notes List
- Created `TransactionalTaskRepository` trait (separate from `TaskRepository` to avoid mockall incompatibility with `&mut Transaction`)
- Implemented `save_in_tx()` and `save_idempotent_in_tx()` on `PostgresTaskRepository` — single INSERT on caller's transaction, no extra locks
- Added `enqueue_in_tx()` and `enqueue_in_tx_idempotent()` to `SchedulerService` with optional `tx_repo` field
- Added `enqueue_in_tx()` and `enqueue_in_tx_idempotent()` to `IronDefer` public API
- Updated architecture boundary comment in `crates/api/src/lib.rs` to allow `sqlx::Transaction` across the public API boundary
- Added `sqlx` dependency to `crates/application/Cargo.toml` for `Transaction`/`Postgres` types
- 4 integration tests: committed tx visible, rollback invisible, idempotency+MVCC isolation, concurrent worker poll during uncommitted window
- Fixed pre-existing `SweeperService::new` call sites missing `idempotency_key_retention` parameter

### File List
- crates/application/Cargo.toml (modified — added sqlx dependency)
- crates/application/src/lib.rs (modified — export TransactionalTaskRepository)
- crates/application/src/ports/mod.rs (modified — export TransactionalTaskRepository)
- crates/application/src/ports/task_repository.rs (modified — added TransactionalTaskRepository trait)
- crates/application/src/services/scheduler.rs (modified — added enqueue_in_tx, enqueue_in_tx_idempotent, with_tx_repo; fixed maybe_idempotency_key usage)
- crates/infrastructure/src/adapters/postgres_task_repository.rs (modified — implemented TransactionalTaskRepository)
- crates/api/src/lib.rs (modified — added enqueue_in_tx, enqueue_in_tx_idempotent; updated arch boundary comment; fixed SweeperService::new call; wired tx_repo in builder)
- crates/api/tests/transactional_enqueue_test.rs (new — 4 integration tests)
- crates/api/tests/sweeper_test.rs (modified — fixed SweeperService::new call)
- .sqlx/ (modified — regenerated offline query cache)

## Change Log
- 2026-04-24: Story 9.2 implementation — transactional enqueue (enqueue_in_tx + enqueue_in_tx_idempotent) across all layers with 4 integration tests

### Review Findings

- [x] [Review][Patch] Misleading Error Context in save_idempotent_in_tx [crates/infrastructure/src/adapters/postgres_task_repository.rs:1213] — Uses new TaskId instead of conflicting ID in NotInExpectedState error.
- [x] [Review][Patch] Semantic Error Conflation in SchedulerService [crates/application/src/services/scheduler.rs:136] — Domain validation errors (empty KIND) reported as generic InvalidPayload.
- [x] [Review][Patch] Scheduler Configuration Safety [crates/application/src/services/scheduler.rs:126] — enqueue_in_tx depends on runtime check for tx_repo; IronDeferBuilder should guarantee it.
- [x] [Review][Patch] Duration Overflow in Expiry Calculation [crates/application/src/services/scheduler.rs:175] — now + retention can overflow if retention is extremely large.
- [x] [Review][Patch] Idempotency Key Length Validation [crates/api/src/lib.rs:396] — Missing length check on idempotency_key before DB insertion.
- [x] [Review][Patch] Opaque Transaction Instrumentation [crates/infrastructure/src/adapters/postgres_task_repository.rs:1055] — Transactional methods lack detailed spans for better correlation.
