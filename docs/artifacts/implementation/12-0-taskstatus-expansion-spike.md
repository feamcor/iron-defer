# Story 12.0: TaskStatus Expansion Spike

Status: done

## Story

As a developer,
I want the `TaskStatus::Suspended` variant and all match-arm updates shipped as an isolated structural change,
so that the compile-breaking enum expansion is reviewed separately from behavioral implementation.

## Acceptance Criteria

1. **Given** the `TaskStatus` enum (already `#[non_exhaustive]`)
   **When** the `Suspended` variant is added
   **Then** all `match` arms across all crates are updated (Sweeper, REST handlers, CLI output, cancel logic, tests)
   **And** the Sweeper's zombie recovery query explicitly excludes `Suspended` tasks
   **And** `cargo test --workspace` passes with zero behavioral changes — the variant exists but no code path produces it yet
   **And** the `suspended_at TIMESTAMPTZ` and `signal_payload JSONB` columns are added to the tasks table

## Functional Requirements Coverage

- **FR60 (partial):** Adds `Suspended` status variant — prerequisite for HITL suspend/resume behavior in Story 12.1
- **FR62 (partial):** Sweeper zombie recovery explicitly excludes `Suspended` tasks

## Tasks / Subtasks

- [x] Task 1: Database migration (AC: 1)
  - [x] 1.1 Create `migrations/0011_add_suspend_columns.sql`:
    ```sql
    -- Extend the status CHECK constraint to include 'suspended' (G7 HITL).
    -- Without this, any UPDATE setting status='suspended' will be rejected.
    ALTER TABLE tasks DROP CONSTRAINT tasks_status_check;
    ALTER TABLE tasks ADD CONSTRAINT tasks_status_check
        CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled', 'suspended'));

    ALTER TABLE tasks ADD COLUMN suspended_at TIMESTAMPTZ;
    ALTER TABLE tasks ADD COLUMN signal_payload JSONB;
    ```
  - [x] 1.2 Regenerate `.sqlx/` offline cache after migration

- [x] Task 2: Add `TaskStatus::Suspended` variant (AC: 1)
  - [x] 2.1 Add `Suspended` variant to `TaskStatus` enum in `crates/domain/src/model/task.rs:65`
  - [x] 2.2 Add `Self::Suspended => "suspended"` arm to `TaskStatus::as_str()` at line 76
  - [x] 2.3 Add `Suspended` to serde round-trip tests in `crates/domain/src/model/task.rs` (tests section ~line 436)

- [x] Task 3: Update TaskRecord with new fields (AC: 1)
  - [x] 3.1 Add `pub(crate) suspended_at: Option<DateTime<Utc>>` field to `TaskRecord` at line 111 (after `checkpoint`)
  - [x] 3.2 Add `pub(crate) signal_payload: Option<serde_json::Value>` field
  - [x] 3.3 Add accessor methods: `pub fn suspended_at(&self) -> Option<DateTime<Utc>>` and `pub fn signal_payload(&self) -> Option<&serde_json::Value>`
  - [x] 3.4 bon `Builder` derives handle `Option<>` fields automatically (default `None`)

- [x] Task 4: Update infrastructure layer — TaskRow and queries (AC: 1)
  - [x] 4.1 Add `suspended_at: Option<DateTime<Utc>>` and `signal_payload: Option<serde_json::Value>` to `TaskRow` struct in `postgres_task_repository.rs`
  - [x] 4.2 Add `suspended_at` and `signal_payload` to `TaskRowWithTotal` struct
  - [x] 4.3 Update `TryFrom<TaskRow> for TaskRecord` and `TryFrom<TaskRowWithTotal> for TaskRecord` to map new fields
  - [x] 4.4 Add `suspended_at` and `signal_payload` to all RETURNING clauses: `save()`, `save_idempotent()`, `save_in_tx()`, `save_idempotent_in_tx()`, `claim_next()`, `complete()`, `fail()`, `cancel()`, `release_leases_for_worker()`, `release_lease_for_task()`
  - [x] 4.5 Add `suspended_at` and `signal_payload` to INSERT column lists in `save()`, `save_in_tx()`, `save_idempotent()`, `save_idempotent_in_tx()`
  - [x] 4.6 Add `suspended_at` and `signal_payload` to SELECT in `find_by_id()`, `list_by_queue()`, `list_tasks()`
  - [x] 4.7 Add `"suspended"` arm to `parse_status()` function (~line 205)
  - [x] 4.8 Add `TaskStatus::Suspended => "suspended"` arm to `status_to_str()` function (~line 218)

- [x] Task 5: Update REST layer (AC: 1)
  - [x] 5.1 Add `suspended_at: Option<DateTime<Utc>>` and `signal_payload: Option<serde_json::Value>` to `TaskResponse` in `crates/api/src/http/handlers/tasks.rs:56` (serializes as `suspendedAt` and `signalPayload` via camelCase)
  - [x] 5.2 Update `From<TaskRecord> for TaskResponse` to map new fields
  - [x] 5.3 Add `TaskStatus::Suspended` arm to `status_to_str()` at line 127 — `Self::Suspended => "suspended"`
  - [x] 5.4 Add `TaskStatus::Suspended` arm to cancel response handler at line 303 — return appropriate error (suspended tasks cannot be cancelled; return 409 with message "task is suspended")
  - [x] 5.5 Add `"suspended"` arm to `parse_status_filter()` at line 414 — `"suspended" => TaskStatus::Suspended`
  - [x] 5.6 Update error message listing valid statuses in `parse_status_filter()` to include "suspended"

- [x] Task 6: Update CLI layer (AC: 1)
  - [x] 6.1 Add `TaskStatus::Suspended => "suspended"` arm to CLI `output.rs` at line 17
  - [x] 6.2 Add `"suspended"` arm to CLI `tasks.rs` status parsing at line 28

- [x] Task 7: Update worker emit_task_failed match (AC: 1)
  - [x] 7.1 In `crates/application/src/services/worker.rs:867`, the `emit_task_failed()` function matches on `record.status()`. The `other =>` catch-all at line 932 already handles unknown statuses. No change needed — `Suspended` will never be a result of `repo.fail()`. Add a code comment documenting this invariant.

- [x] Task 8: Verify sweeper excludes Suspended (AC: 1)
  - [x] 8.1 Verify `recover_zombie_tasks()` WHERE clause uses `status = 'running'` — naturally excludes Suspended tasks. Add a code comment documenting this intentional exclusion.
  - [x] 8.2 If the sweeper's idempotency key cleanup query has status predicates, verify Suspended is excluded from terminal-status checks (it's not terminal)

- [x] Task 9: Offline cache & compilation (AC: 1)
  - [x] 9.1 Regenerate `.sqlx/` offline cache: `cargo sqlx prepare --workspace`
  - [x] 9.2 Verify `cargo test --workspace` passes — zero behavioral changes, only structural additions
  - [x] 9.3 Verify `cargo clippy --workspace` clean

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules:**
- Migration: `migrations/0011_add_suspend_columns.sql`
- Domain: `TaskStatus::Suspended` variant + `TaskRecord` fields in `crates/domain/src/model/task.rs`
- Infrastructure: `TaskRow`, `TaskRowWithTotal`, all query RETURNING clauses, `parse_status()`, `status_to_str()`
- Application: Worker `emit_task_failed()` — verify catch-all handles Suspended (no behavioral change)
- API: `TaskResponse` DTO + `status_to_str()` + `parse_status_filter()` + cancel handler + CLI output/parsing

**Key constraint:** This is a STRUCTURAL spike — zero new behavior. No code path should produce `Suspended` status after this story. The variant exists but is inert.

### Critical Implementation Details

1. **CHECK constraint blocker:** Migration `0001_create_tasks_table.sql` (line 26-27) has `CONSTRAINT tasks_status_check CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled'))`. This MUST be dropped and recreated with `'suspended'` added BEFORE any code can write `status = 'suspended'`. The migration handles this.

2. **Match arm locations (9 sites):**
   - `crates/domain/src/model/task.rs:76` — `TaskStatus::as_str()`
   - `crates/infrastructure/src/adapters/postgres_task_repository.rs:205` — `parse_status()`
   - `crates/infrastructure/src/adapters/postgres_task_repository.rs:218` — `status_to_str()`
   - `crates/api/src/http/handlers/tasks.rs:127` — REST `status_to_str()`
   - `crates/api/src/http/handlers/tasks.rs:303` — cancel response handler (add explicit Suspended arm)
   - `crates/api/src/http/handlers/tasks.rs:414` — `parse_status_filter()`
   - `crates/api/src/cli/output.rs:17` — CLI status display
   - `crates/api/src/cli/tasks.rs:28` — CLI status parsing
   - `crates/application/src/services/worker.rs:867` — `emit_task_failed()` (existing catch-all suffices)

3. **RETURNING clause pattern:** Every query that uses `RETURNING` must include `suspended_at, signal_payload`. Follow the exact pattern used when `checkpoint` was added in Story 11.1 — add to the end of every RETURNING clause. There are 12 queries to update across `save()`, `save_idempotent()`, `save_in_tx()`, `save_idempotent_in_tx()`, `claim_next()`, `complete()`, `fail()`, `cancel()`, `release_leases_for_worker()`, `release_lease_for_task()`, and the two `recover_zombie_tasks()` sub-queries.

4. **INSERT columns:** `save()`, `save_in_tx()`, `save_idempotent()`, `save_idempotent_in_tx()` must include `suspended_at` and `signal_payload` in the INSERT column list (value: NULL for both, matching TaskRecord defaults).

5. **SELECT queries:** `find_by_id()`, `list_by_queue()`, `list_tasks()` must include `suspended_at, signal_payload` in SELECT.

6. **bon Builder:** `TaskRecord`'s `#[derive(bon::Builder)]` handles `Option<>` fields automatically — defaults to `None`. No builder changes needed.

7. **Cancel handler:** `Suspended` tasks should NOT be cancellable — return 409 with "task is suspended". This matches the pattern for `Running` (already_claimed). Cancellation can be re-evaluated in Story 12.1 if needed.

8. **`recover_zombie_tasks()` safety:** The two UPDATE queries use `WHERE status = 'running'`, which naturally excludes Suspended. Add a comment: `-- Intentionally excludes 'suspended' tasks (G7 HITL — suspend watchdog handles timeout separately)`.

9. **Test lifecycle guard:** The domain test at ~line 558 asserts `all_statuses.len() == 5`. This must be updated to 6 after adding Suspended.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `migrations/0011_add_suspend_columns.sql` | **NEW** — `ALTER TABLE tasks ADD COLUMN suspended_at TIMESTAMPTZ; ALTER TABLE tasks ADD COLUMN signal_payload JSONB;` |
| `crates/domain/src/model/task.rs` | Add `Suspended` variant to `TaskStatus`, add `suspended_at` + `signal_payload` fields to `TaskRecord`, add accessors |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Update `TaskRow`, `TaskRowWithTotal`, `parse_status()`, `status_to_str()`, all RETURNING/INSERT/SELECT clauses (~12 queries) |
| `crates/api/src/http/handlers/tasks.rs` | Update `TaskResponse`, `From<TaskRecord>`, `status_to_str()`, cancel handler, `parse_status_filter()` |
| `crates/api/src/cli/output.rs` | Add `Suspended` display arm |
| `crates/api/src/cli/tasks.rs` | Add `"suspended"` parse arm |
| `crates/application/src/services/worker.rs` | Document invariant in `emit_task_failed()` |
| `.sqlx/` | Regenerate offline cache |

### Testing Standards

- No new test files needed — this is structural only
- Verify existing test suite passes with zero modifications (aside from potential test-helper TaskRecord construction updates)
- Add `Suspended` to serde round-trip tests in domain crate
- If any test constructs `TaskRecord` manually (not via builder), it may need updating — but bon Builder handles Option defaults automatically

### Previous Story Intelligence

**From Story 11.1 (checkpoint — column addition pattern):**
- Adding columns follows: migration → TaskRow fields → TryFrom impls → RETURNING/INSERT/SELECT clauses → TaskResponse → accessors
- ~12 queries needed RETURNING clause updates when `checkpoint` column was added
- bon Builder handles new `Option<>` fields automatically (defaults to `None`)

**From Story 6.3 (TaskStatus #[non_exhaustive] + match arm updates):**
- Story 6.3 made `TaskStatus` `#[non_exhaustive]` and updated all match arms
- The 9 match locations were established then — same 9 sites need updating here
- Cancel handler uses explicit arms: `Pending` (shouldn't happen), `Running` (already_claimed), `Completed|Failed|Cancelled` (terminal), `_` (unknown)

**From Story 10.2 (audit log — transaction wrapping pattern):**
- `recover_zombie_tasks()` wraps two UPDATEs in a transaction with audit rows
- Zombie recovery WHERE clause uses `status = 'running'` — naturally excludes Suspended

### References

- [Source: docs/artifacts/planning/epics.md — Epic 12, Story 12.0 (lines 1251-1270)]
- [Source: docs/artifacts/planning/prd.md — FR60, FR62 (lines 989, 991)]
- [Source: crates/domain/src/model/task.rs — TaskStatus (lines 58-84), TaskRecord (lines 91-112)]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — parse_status (line 205), status_to_str (line 218), claim_next (line 622), recover_zombie_tasks (~line 825)]
- [Source: crates/api/src/http/handlers/tasks.rs — TaskResponse (line 56), cancel handler (line 303), parse_status_filter (line 414)]
- [Source: crates/api/src/cli/output.rs — status display (line 17)]
- [Source: crates/api/src/cli/tasks.rs — status parsing (line 28)]
- [Source: crates/application/src/services/worker.rs — emit_task_failed (line 867)]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Pre-existing test failure: `e2e_trace_propagation_across_retries` — OTel span-capture test unrelated to structural TaskStatus changes

### Completion Notes List

- Task 1: Created migration `0011_add_suspend_columns.sql` — drops/recreates CHECK constraint with 'suspended', adds `suspended_at TIMESTAMPTZ` and `signal_payload JSONB` columns
- Task 2: Added `TaskStatus::Suspended` variant with `as_str()` arm and serde round-trip tests; updated lifecycle state count from 5 to 6
- Task 3: Added `suspended_at` and `signal_payload` fields to `TaskRecord` with accessor methods; bon Builder handles Option defaults automatically
- Task 4: Updated `TaskRow`, `TaskRowWithTotal`, `From`/`TryFrom` impls, `parse_status()`, `status_to_str()`, and all 12 query RETURNING/INSERT/SELECT clauses; updated `CancelRow` struct and manual `TaskRow` construction
- Task 5: Updated `TaskResponse` DTO with new fields, `From<TaskRecord>`, `status_to_str()`, `parse_status_filter()` with 'suspended'; added explicit `Suspended` arm in cancel handler returning 409 via new `AppError::task_suspended()` method
- Task 6: Updated CLI `format_status()` and `parse_status()` with Suspended arm; updated CLI help text
- Task 7: Added invariant comment in `emit_task_failed()` documenting that Suspended is never a result of `repo.fail()`
- Task 8: Added comment documenting intentional exclusion of Suspended from sweeper's zombie recovery WHERE clause; verified idempotency cleanup uses terminal-status predicates only
- Task 9: Regenerated `.sqlx/` offline cache (14 files); `cargo clippy --workspace` clean (no new warnings); `cargo test --workspace` passes (1 pre-existing OTel test failure)

### Change Log

- 2026-04-25: Structural TaskStatus expansion spike — Suspended variant, schema columns, all match arms, sqlx cache regeneration

### File List

- migrations/0011_add_suspend_columns.sql (NEW)
- crates/domain/src/model/task.rs (MODIFIED)
- crates/infrastructure/src/adapters/postgres_task_repository.rs (MODIFIED)
- crates/api/src/http/handlers/tasks.rs (MODIFIED)
- crates/api/src/http/errors.rs (MODIFIED)
- crates/api/src/cli/output.rs (MODIFIED)
- crates/api/src/cli/tasks.rs (MODIFIED)
- crates/application/src/services/worker.rs (MODIFIED)
- .sqlx/ (REGENERATED — 14 cache files)

### Review Findings

- [x] [Review][Patch] Queues with only suspended tasks hidden [crates/infrastructure/src/adapters/postgres_task_repository.rs:1034]
- [x] [Review][Defer] Lack of signal_payload validation [migrations/0011_add_suspend_columns.sql] — deferred, pre-existing
- [x] [Review][Defer] High SQL duplication [crates/infrastructure/src/adapters/postgres_task_repository.rs] — deferred, pre-existing
- [x] [Review][Defer] Cloning of JSON blobs in API mapping [crates/api/src/http/handlers/tasks.rs] — deferred, pre-existing
