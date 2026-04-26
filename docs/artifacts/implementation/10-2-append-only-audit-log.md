# Story 10.2: Append-Only Audit Log

Status: done

## Story

As a compliance officer,
I want every task state transition recorded in a tamper-evident, append-only audit log table,
so that I can produce complete lifecycle evidence for PCI DSS, SOC 2, and DORA audits.

## Acceptance Criteria

1. **Given** a task that transitions through states (Pending→Running→Completed)
   **When** each transition occurs
   **Then** a new row is inserted into `task_audit_log` with: task_id, from_status, to_status, timestamp, worker_id, trace_id, metadata
   **And** the audit insert is in the same database transaction as the state change (NFR-C2)

2. **Given** the `task_audit_log` table
   **When** any UPDATE or DELETE is attempted
   **Then** the database-level `BEFORE UPDATE OR DELETE` trigger raises an exception (NFR-C1)

3. **Given** an operator querying audit history
   **When** `SELECT * FROM task_audit_log WHERE task_id = $1 ORDER BY timestamp` is executed
   **Then** the complete lifecycle of the task is returned

4. **Given** transactional enqueue (G2) with audit logging enabled
   **When** the caller's transaction rolls back
   **Then** both the task row and its creation audit entry are erased (no phantom audit rows)

5. **Given** UNLOGGED mode and audit logging configured simultaneously
   **When** the engine starts
   **Then** startup is rejected with a clear error (mutual exclusion — already enforced in `DatabaseConfig::validate()`)

## Functional Requirements Coverage

- **FR55:** Compliance audit trail — every state transition produces an immutable audit row
- **FR56:** Evidence reconstruction — complete lifecycle queryable per task
- **NFR-C1:** Append-only enforcement via database trigger (not application-level only)
- **NFR-C2:** Audit insert atomic with state change (same transaction)

## Tasks / Subtasks

- [x] Task 1: Database migration — audit log table (AC: 1, 2, 3)
  - [x] 1.1 Create `migrations/0006_create_audit_log_table.sql` with:
    - `task_audit_log` table: `id BIGSERIAL PRIMARY KEY`, `task_id UUID NOT NULL REFERENCES tasks(id)`, `from_status TEXT`, `to_status TEXT NOT NULL`, `timestamp TIMESTAMPTZ NOT NULL DEFAULT now()`, `worker_id UUID`, `trace_id VARCHAR`, `metadata JSONB`
    - Note: `from_status` is nullable for the initial creation transition (NULL→Pending)
  - [x] 1.2 Add index: `CREATE INDEX idx_audit_log_task_time ON task_audit_log (task_id, timestamp)`
  - [x] 1.3 Add immutability trigger function: `audit_log_immutable()` that `RAISE EXCEPTION 'audit log is append-only: % operations are forbidden', TG_OP`
  - [x] 1.4 Add trigger: `BEFORE UPDATE OR DELETE ON task_audit_log FOR EACH ROW EXECUTE FUNCTION audit_log_immutable()`
  - [x] 1.5 Regenerate `.sqlx/` offline cache: `cargo sqlx prepare --workspace`

- [x] Task 2: Repository — add `audit_log` flag (AC: 1)
  - [x] 2.1 Add `audit_log: bool` field to `PostgresTaskRepository` struct (line 227)
  - [x] 2.2 Update constructor: `pub fn new(pool: PgPool, audit_log: bool) -> Self`
  - [x] 2.3 Update ALL call sites of `PostgresTaskRepository::new()`:
    - `crates/api/src/lib.rs` line 1075 (`IronDeferBuilder::build()`): pass `self.database_config.audit_log`
    - `crates/api/src/lib.rs` line 487 (`IronDefer::start()`): **PROBLEM** — `IronDefer` struct does not store `database_config`. Fix: add `audit_log: bool` field to `IronDefer` struct, populate it in `build()`, use it in `start()` when constructing the repository
    - Any test files that construct `PostgresTaskRepository::new(pool)` directly — pass `false` for tests not testing audit
  - [x] 2.4 Add private helper method: `async fn insert_audit_row(&self, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, task_id: Uuid, from_status: Option<&str>, to_status: &str, worker_id: Option<Uuid>, trace_id: Option<&str>, metadata: Option<serde_json::Value>) -> Result<(), TaskError>` — no-op when `self.audit_log == false`

- [x] Task 3: Wrap `save()` in a transaction with audit insert (AC: 1, 4)
  - [x] 3.1 Convert `save()` from `fetch_one(&self.pool)` to transaction-based: `pool.begin()` → INSERT task → insert audit row (NULL→Pending) → `tx.commit()`
  - [x] 3.2 Audit row: `from_status = NULL`, `to_status = 'pending'`, `worker_id = NULL`, `trace_id` from task record, `metadata = NULL`
  - [x] 3.3 When `self.audit_log == false`, skip audit insert (existing single-query behavior preserved)

- [x] Task 4: Wrap `save_idempotent()` with audit insert (AC: 1, 4)
  - [x] 4.1 `save_idempotent()` already uses `pool.begin()` — add audit insert after successful INSERT (when `created = true`)
  - [x] 4.2 Skip audit insert when `created = false` (duplicate — no state transition occurred)
  - [x] 4.3 Audit row: `from_status = NULL`, `to_status = 'pending'`, same pattern as `save()`

- [x] Task 5: Wrap `claim_next()` in a transaction with audit insert (AC: 1)
  - [x] 5.1 Convert `claim_next()` from `fetch_optional(&self.pool)` to transaction-based
  - [x] 5.2 After UPDATE (Pending→Running), insert audit row: `from_status = 'pending'`, `to_status = 'running'`, `worker_id` from parameter, `trace_id` from returned row
  - [x] 5.3 When `self.audit_log == false`, skip — single-query behavior preserved

- [x] Task 6: Wrap `complete()` in a transaction with audit insert (AC: 1)
  - [x] 6.1 Convert `complete()` from `fetch_optional(&self.pool)` to transaction-based
  - [x] 6.2 After UPDATE (Running→Completed), insert audit row: `from_status = 'running'`, `to_status = 'completed'`, `worker_id` from task's `claimed_by`, `trace_id` from task record
  - [x] 6.3 When `self.audit_log == false`, skip — single-query behavior preserved

- [x] Task 7: Wrap `fail()` in a transaction with audit insert (AC: 1)
  - [x] 7.1 Convert `fail()` from `fetch_optional(&self.pool)` to transaction-based
  - [x] 7.2 After UPDATE, determine actual transition from RETURNING row:
    - If returned `status = 'pending'` → audit: `from_status = 'running'`, `to_status = 'pending'` (retryable)
    - If returned `status = 'failed'` → audit: `from_status = 'running'`, `to_status = 'failed'` (exhausted)
  - [x] 7.3 Include `error_message` in audit `metadata` JSONB: `{"error": "truncated error message"}`
  - [x] 7.4 When `self.audit_log == false`, skip — single-query behavior preserved

- [x] Task 8: Add audit inserts to `recover_zombie_tasks()` (AC: 1)
  - [x] 8.1 `recover_zombie_tasks()` already uses `pool.begin()` — add audit inserts after each UPDATE batch
  - [x] 8.2 For retryable rows: audit `from_status = 'running'`, `to_status = 'pending'`, `worker_id = NULL` (sweeper, not a worker)
  - [x] 8.3 For exhausted rows: audit `from_status = 'running'`, `to_status = 'failed'`, `metadata = {"error": "lease expired: max attempts exhausted"}`
  - [x] 8.4 Need to read `trace_id` from each recovered row — UPDATE RETURNING must include `trace_id` (Story 10.1 will have added this column)
  - [x] 8.5 When `self.audit_log == false`, skip audit inserts

- [x] Task 9: Wrap `cancel()` with audit insert (AC: 1)
  - [x] 9.1 `cancel()` currently uses a CTE (atomic at DB level but not a Rust transaction). Wrap in `pool.begin()`: run the CTE against `&mut *tx`, then conditionally insert audit row, then `tx.commit()` (Option A — consistent with other methods)
  - [x] 9.2 Prerequisite: the local `CancelRow` struct (line 860) must already include `trace_id: Option<String>` (added in Story 10.1 Task 4.7). Read `trace_id` from `row.trace_id` for the audit row.
  - [x] 9.3 Audit row: `from_status = 'pending'`, `to_status = 'cancelled'`, `worker_id = NULL` (cancellation is an operator action), `trace_id` from CancelRow
  - [x] 9.4 Only insert audit row when `was_cancelled = true` (not for NotFound or NotCancellable)
  - [x] 9.5 When `self.audit_log == false`, skip

- [x] Task 10: Wrap `release_leases_for_worker()` and `release_lease_for_task()` with audit (AC: 1)
  - [x] 10.1 These transition Running→Pending during graceful shutdown
  - [x] 10.2 `release_leases_for_worker()` currently uses `RETURNING id` only. Extend RETURNING to include `trace_id` so audit rows can populate `trace_id`.
  - [x] 10.3 `release_lease_for_task()` currently uses `.execute()` with no RETURNING. Change to `fetch_optional` with `RETURNING id, trace_id` to get `trace_id` for the audit row. (`task_id` is already available from the method parameter.)
  - [x] 10.4 Insert audit rows: `from_status = 'running'`, `to_status = 'pending'`, `worker_id` from parameter (for `release_leases_for_worker`) or NULL, `metadata = {"reason": "lease released: graceful shutdown"}`
  - [x] 10.5 When `self.audit_log == false`, skip

- [x] Task 11: REST API — audit log query endpoint (AC: 3)
  - [x] 11.1 Add `GET /tasks/{id}/audit` endpoint in `crates/api/src/http/handlers/tasks.rs`
  - [x] 11.2 Returns array of audit log entries ordered by timestamp
  - [x] 11.3 Response DTO: `AuditLogEntry { id, task_id, from_status, to_status, timestamp, worker_id, trace_id, metadata }`
  - [x] 11.4 Add query method to `TaskRepository` trait: `async fn audit_log(&self, task_id: TaskId) -> Result<Vec<AuditLogEntry>, TaskError>`
  - [x] 11.5 Implement in `PostgresTaskRepository`: `SELECT * FROM task_audit_log WHERE task_id = $1 ORDER BY timestamp ASC`
  - [x] 11.6 When `self.audit_log == false`, return empty vec (or specific error indicating audit log is disabled)
  - [x] 11.7 Add OpenAPI annotations (`#[utoipa::path]`)

- [x] Task 12: Domain — audit log entry type (AC: 3)
  - [x] 12.1 Add `AuditLogEntry` struct to `crates/domain/src/model/` (new file or in `task.rs`)
  - [x] 12.2 Fields: `id: i64`, `task_id: TaskId`, `from_status: Option<String>`, `to_status: String`, `timestamp: DateTime<Utc>`, `worker_id: Option<WorkerId>`, `trace_id: Option<String>`, `metadata: Option<serde_json::Value>`
  - [x] 12.3 Export from `domain` crate

- [x] Task 13: Integration tests (AC: 1, 2, 3, 5)
  - [x] 13.1 Create `crates/api/tests/audit_log_test.rs` (naming note: `audit_trail_test.rs` already exists — that's for OTel/structured logging, NOT the DB audit log; use a distinct name)
  - [x] 13.2 Test: submit task → claim → complete → query audit log → verify 3 rows: (NULL→pending, pending→running, running→completed)
  - [x] 13.3 Test: submit task → claim → fail (retry) → re-claim → complete → verify 5 rows with correct transitions
  - [x] 13.4 Test: immutability trigger — attempt UPDATE on `task_audit_log` → verify Postgres error
  - [x] 13.5 Test: immutability trigger — attempt DELETE on `task_audit_log` → verify Postgres error
  - [x] 13.6 Test: cancel pending task → verify audit row (pending→cancelled)
  - [x] 13.7 Test: `audit_log = false` → submit + complete → verify no audit rows in table
  - [x] 13.8 Test: trace_id correlation — submit with trace_id, complete → verify trace_id populated in all audit rows
  - [x] 13.9 Test: atomicity cross-reference — submit N tasks, complete all, query both `tasks` and `task_audit_log`. Assert every task with a non-pending status has corresponding audit rows. No committed state change without an audit row.
  - [x] 13.10 Unique queue names per test for isolation

- [x] Task 14: Update existing test infrastructure (AC: backward compatibility)
  - [x] 14.1 Update all `PostgresTaskRepository::new(pool)` call sites in tests to `PostgresTaskRepository::new(pool, false)` (audit disabled by default in tests unless explicitly testing audit)
  - [x] 14.2 Verify all existing tests pass with the constructor change
  - [x] 14.3 `MockTaskRepository` auto-generated mock (`#[automock]`) automatically gains the new `audit_log()` method — no manual action needed. But any new integration test that calls the mock via the REST audit endpoint must set `.expect_audit_log()` explicitly.

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules (enforced by Cargo crate boundaries):**
- `domain` ← `AuditLogEntry` struct lives here (no external dependencies)
- `application` ← `TaskRepository` trait gains `audit_log()` method; no audit logic here
- `infrastructure` ← All audit INSERT logic lives in `PostgresTaskRepository`; conditional on `audit_log` flag
- `api` ← REST endpoint wiring + response DTO

**Key design:** Audit logging is infrastructure-only. The application layer is unaware of audit — it calls the same `complete()`, `fail()`, etc. methods. The repository conditionally inserts audit rows based on the `audit_log` flag.

### Critical Transaction Pattern

**Every state-transition method** must be wrapped in a transaction when `audit_log == true`:

```rust
async fn complete(&self, task_id: TaskId) -> Result<TaskRecord, TaskError> {
    if self.audit_log {
        let mut tx = self.pool.begin().await.map_err(PostgresAdapterError::from)?;
        let row = sqlx::query_as!(TaskRow, "UPDATE tasks SET status = 'completed' ... WHERE id = $1 AND status = 'running' RETURNING ...", task_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(PostgresAdapterError::from)?;
        let row = match row {
            Some(r) => r,
            None => return Err(TaskError::NotInExpectedState { task_id, expected: "Running" }),
        };
        self.insert_audit_row(&mut tx, row.id, Some("running"), "completed", row.claimed_by, row.trace_id.as_deref(), None).await?;
        tx.commit().await.map_err(PostgresAdapterError::from)?;
        Ok(TaskRecord::try_from(row)?)
    } else {
        // Existing single-query path (no transaction overhead)
        let row = sqlx::query_as!(TaskRow, "... RETURNING ...", task_id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(PostgresAdapterError::from)?;
        match row {
            Some(r) => Ok(TaskRecord::try_from(r)?),
            None => Err(TaskError::NotInExpectedState { task_id, expected: "Running" }),
        }
    }
}
```

**Alternative approach (simpler, preferred):** Always use a transaction, but only INSERT audit when `self.audit_log`. This avoids code duplication but adds minor transaction overhead for non-audit mode. Evaluate trade-off during implementation — the simpler code is likely worth the ~0.1ms overhead.

### State Transition Map (all audit insert points)

| Method | Line | From | To | worker_id | Notes |
|--------|------|------|----|-----------|-------|
| `save()` | 245 | NULL | pending | NULL | Initial creation |
| `save_idempotent()` | 308 | NULL | pending | NULL | Only when `created = true` |
| `claim_next()` | 492 | pending | running | parameter | Worker claims task |
| `complete()` | 539 | running | completed | from `claimed_by` | Normal completion |
| `fail()` | 571 | running | pending OR failed | from `claimed_by` | Retry or exhausted |
| `recover_zombie_tasks()` | 645 | running | pending | NULL (sweeper) | Retryable zombies |
| `recover_zombie_tasks()` | 645 | running | failed | NULL (sweeper) | Exhausted zombies |
| `cancel()` | 858 | pending | cancelled | NULL (operator) | Operator cancellation |
| `release_leases_for_worker()` | ~952 | running | pending | from parameter | Graceful shutdown |
| `release_lease_for_task()` | ~980 | running | pending | NULL | Claimed but undispatchable |

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `migrations/0006_create_audit_log_table.sql` | **NEW** — table, trigger, index |
| `crates/domain/src/model/audit.rs` | **NEW** — `AuditLogEntry` struct |
| `crates/domain/src/model/mod.rs` | Add `mod audit; pub use audit::*;` |
| `crates/domain/src/lib.rs` | Re-export `AuditLogEntry` |
| `crates/application/src/ports/task_repository.rs` | Add `audit_log()` method to trait |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Add `audit_log` flag + `insert_audit_row()` helper + wrap all 9 state-transition methods |
| `crates/api/src/http/handlers/tasks.rs` | Add `GET /tasks/{id}/audit` endpoint + `AuditLogResponse` DTO |
| `crates/api/src/http/router.rs` | Register audit endpoint route |
| `crates/api/src/lib.rs` | Thread `audit_log` flag to repository constructor |
| `crates/api/tests/audit_log_test.rs` | **NEW** — integration tests |

### Testing Standards

- Integration tests in `crates/api/tests/` as flat files
- Use `fresh_pool_on_shared_container()` for clean DB state per test
- Unique queue names per test (e.g., `"audit_lifecycle"`, `"audit_immutable"`)
- Assert DB state directly — query `task_audit_log` table to verify row count and transition values
- Immutability tests: use raw `sqlx::query!("UPDATE task_audit_log SET ...")` and assert Postgres error
- Note: `audit_trail_test.rs` already exists (Story 3.3 — OTel structured logging). Name the new file `audit_log_test.rs` to avoid confusion

### Critical Constraints

1. **Same transaction, same connection** (NFR-C2): Audit INSERT must use `&mut *tx`, not `&self.pool`. A separate connection breaks atomicity — if the state UPDATE commits but the audit INSERT fails on a different connection, you have a state change without an audit row.

2. **RETURNING columns**: All UPDATE queries already use `RETURNING`. When `audit_log == true`, the returned row provides `trace_id` (from Story 10.1) and `claimed_by` for the audit row. After Story 10.1, all RETURNING clauses include `trace_id`.

3. **`recover_zombie_tasks()` needs trace_id**: Currently the sweeper's UPDATE RETURNING only includes `id, queue, kind`. When audit is enabled, it needs `trace_id` and `claimed_by` too. Extend the RETURNING clause.

4. **Trigger enforcement is database-level** (NFR-C1): The `BEFORE UPDATE OR DELETE` trigger prevents ALL modifications, including from admin SQL sessions. This is by design for compliance.

5. **No `audit_log` table modifications**: The codebase must contain ZERO `UPDATE` or `DELETE` queries against `task_audit_log`. Code review should verify this.

6. **`cancel()` CTE needs restructuring**: Currently a single CTE query. To add audit within the same transaction, either: (a) wrap in `pool.begin()` + CTE + audit INSERT + `tx.commit()`, or (b) extend the CTE with a second step that inserts audit. Option (a) is simpler and consistent with other methods.

7. **Migration numbering**: Next migration is `0006_*` (after `0005_add_trace_id_column.sql` from Story 10.1). Story 10.1 MUST be implemented first.

8. **`.sqlx/` offline cache**: Must be regenerated after migration + query changes.

9. **`#[instrument]` on all new public async methods** — skip `self` and `payload`.

10. **Partition-ready schema**: Use `BIGSERIAL` and `TIMESTAMPTZ` for future partitioning by timestamp. Do NOT add partitioning now — it's a future optimization.

11. **`from_status` is nullable**: The initial task creation (NULL→Pending) has no `from_status`. All other transitions have both.

12. **camelCase JSON fields** (ADR-0006): Response DTO uses `fromStatus`, `toStatus`, `workerId`, `traceId`.

### Previous Story Intelligence

**From Story 10.1 (previous story in this epic):**
- `trace_id` column added to tasks table (migration 0005) — audit log can reference it
- All RETURNING clauses in postgres_task_repository.rs will include `trace_id` after 10.1
- `TaskRow` struct will have `trace_id: Option<String>` field
- Story 10.1 adds OTel Events for state transitions — audit log is the durable complement

**From Story 9.1 (completed):**
- `save_idempotent()` already uses `pool.begin()` — serves as pattern for wrapping other methods
- `recover_zombie_tasks()` already uses `pool.begin()` — audit inserts add to existing transaction
- `TaskRow` and `TaskRowWithTotal` need trace_id after Story 10.1, and audit rows reference the same field
- bon::Builder on `TaskRecord` handles new fields gracefully

**From Epic 6 (type hardening):**
- `CancelRow` in `cancel()` is a custom struct — when extending, add trace_id field to it
- `RecoveryOutcome` enum — no changes needed, audit is orthogonal

### Existing Infrastructure to Reuse

- `DatabaseConfig.audit_log: bool` — already exists in `crates/application/src/config.rs` (line 32), defaults to `false`, validation with `unlogged_tables` already implemented
- `PostgresAdapterError::from(sqlx::Error)` — reuse for all new error paths
- `status_to_str()` — reuse for mapping `TaskStatus` to string in audit rows
- `fresh_pool_on_shared_container()` — reuse for integration test setup

### G2 Interaction (Transactional Enqueue — Story 9.2)

Story 9.2 introduces `enqueue_in_tx()` where the caller provides their own transaction. When combined with audit logging:
- The audit INSERT for task creation must go inside the **caller's** transaction
- If the caller rolls back, both the task row AND the audit entry are erased
- This prevents phantom audit rows for tasks that never existed

If Story 9.2 ships before 10.2: the `save_in_tx()` method must also conditionally insert audit. If Story 10.2 ships first: document that 9.2 must add audit support when it lands.

### Project Structure Notes

- New domain type `AuditLogEntry` follows existing pattern: private fields with typed accessors, `#[non_exhaustive]`
- REST endpoint follows existing pattern: handler function in `tasks.rs`, route in `router.rs`
- Migration file naming: `0006_create_audit_log_table.sql`
- Config env var: `IRON_DEFER__DATABASE__AUDIT_LOG=true` (already supported by figment env overlay)

### References

- [Source: docs/artifacts/planning/epics.md — Epic 10, Story 10.2 (lines 1069-1104)]
- [Source: docs/artifacts/planning/prd.md — §G5 Append-only audit log table (lines 167-173)]
- [Source: docs/artifacts/planning/prd.md — FR55, FR56]
- [Source: docs/artifacts/planning/architecture.md — §Audit Log Integration G5, Write Path, Schema Evolution (lines 2008-2040)]
- [Source: crates/application/src/config.rs — DatabaseConfig.audit_log (line 32), validation (line 104)]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — PostgresTaskRepository (line 227), save() (line 245), save_idempotent() (line 308), claim_next() (line 492), complete() (line 539), fail() (line 571), recover_zombie_tasks() (line 645), cancel() (line 858)]
- [Source: crates/application/src/ports/task_repository.rs — TaskRepository trait (line 26)]
- [Source: docs/artifacts/implementation/10-1-otel-distributed-traces.md — Story 10.1 patterns]
- [Source: docs/artifacts/implementation/9-1-idempotency-key-schema-and-submission.md — Transaction patterns, test patterns]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

- All 14 tasks complete: migration, domain type, repository audit_log flag, insert_audit_row helper, save/save_idempotent/claim_next/complete/fail/recover_zombie_tasks/cancel/release_leases wrapping, REST endpoint, integration tests, existing test infrastructure update
- Migration numbered 0007 (0006 was already taken by trace_id index)
- Used "always transaction" approach for state-transition methods — insert_audit_row no-ops when audit_log=false, so zero SQL overhead in non-audit mode
- 8 integration tests covering lifecycle, retry, immutability (UPDATE/DELETE triggers), cancel, audit-disabled mode, trace_id correlation, and atomicity cross-check
- Pre-existing flaky test in otel_traces_test.rs (timing-dependent, passes 2/3 runs independently) — not related to this story

### Change Log

- 2026-04-24: Implemented Story 10.2 — append-only audit log with 14 tasks, 8 integration tests

### File List

- `migrations/0007_create_audit_log_table.sql` — NEW: audit log table, index, immutability trigger
- `crates/domain/src/model/audit.rs` — NEW: AuditLogEntry domain type
- `crates/domain/src/model/mod.rs` — Added audit module export
- `crates/domain/src/lib.rs` — Re-exported AuditLogEntry
- `crates/application/src/ports/task_repository.rs` — Added audit_log() method to TaskRepository trait
- `crates/application/src/services/scheduler.rs` — Added audit_log() proxy method
- `crates/application/src/services/worker.rs` — Added audit_log() stub to StuckClaimRepo test impl
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — Added audit_log flag, insert_audit_row helper, wrapped 9 state-transition methods + audit_log query
- `crates/api/src/lib.rs` — Added audit_log field to IronDefer, threaded through builder, added audit_log() method
- `crates/api/src/http/handlers/tasks.rs` — Added GET /tasks/{id}/audit endpoint + AuditLogResponse DTO
- `crates/api/src/http/router.rs` — Registered audit endpoint route + OpenAPI schema
- `crates/api/src/cli/submit.rs` — Updated PostgresTaskRepository::new() call
- `crates/api/src/cli/workers.rs` — Updated PostgresTaskRepository::new() call
- `crates/api/src/cli/tasks.rs` — Updated PostgresTaskRepository::new() call
- `crates/api/tests/audit_log_test.rs` — NEW: 8 integration tests
- `crates/infrastructure/tests/task_repository_test.rs` — Updated PostgresTaskRepository::new() calls
- `crates/api/tests/sweeper_test.rs` — Updated PostgresTaskRepository::new() calls
- `crates/api/tests/sweeper_counter_test.rs` — Updated PostgresTaskRepository::new() calls
- `crates/api/tests/idempotency_test.rs` — Updated PostgresTaskRepository::new() calls
- `crates/api/tests/cli_test.rs` — Updated PostgresTaskRepository::new() calls
- `crates/api/tests/chaos_worker_crash_test.rs` — Updated PostgresTaskRepository::new() calls
- `.sqlx/` — Regenerated offline cache

### Review Findings

- [ ] [Review][Decision] Audit Bypass via Direct SQL — State transitions are audited via application logic, not database triggers on the tasks table. Direct SQL updates will bypass the audit trail.
- [x] [Review][Patch] Lost worker attribution in terminal transitions [crates/infrastructure/src/adapters/postgres_task_repository.rs:561]
- [x] [Review][Patch] Unbounded Metadata Size [crates/infrastructure/src/adapters/postgres_task_repository.rs:241]
- [x] [Review][Patch] API Redundancy and Race Condition [crates/api/src/http/handlers/tasks.rs:378]
- [x] [Review][Defer] Sequential Audit Inserts in Batches [crates/infrastructure/src/adapters/postgres_task_repository.rs:699] — deferred, pre-existing
- [x] [Review][Defer] Missing Pagination in Audit API [crates/api/src/http/handlers/tasks.rs:373] — deferred, pre-existing
- [x] [Review][Patch] Trace ID column lacks length constraint [migrations/0007_create_audit_log_table.sql:11]
- [x] [Review][Patch] Stringly-typed statuses in AuditLogEntry [crates/domain/src/model/audit.rs:10]
- [x] [Review][Defer] AuditLogEntry constructor bloat [crates/domain/src/model/audit.rs:19] — deferred, pre-existing
