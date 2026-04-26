# Story 6.3: Cancel Path & TaskStatus Hardening

Status: done

## Story

As a developer,
I want the cancel operation to be atomic and the TaskStatus enum to be future-proof,
so that concurrent cancellations cannot corrupt task state and adding new status variants does not break downstream consumers.

## Acceptance Criteria

1. **Cancel SQL atomicity (CR1)**

   **Given** the cancel SQL in `postgres_task_repository.rs`
   **When** a `DELETE /tasks/{id}` request is processed
   **Then** the status check and update are wrapped in a single CTE or transaction that atomically verifies `status = 'pending'` and transitions to `cancelled`
   **And** a concurrent cancel for the same task cannot produce a double-cancellation or inconsistent state (TOCTOU eliminated)

2. **TaskStatus `#[non_exhaustive]` (CR12)**

   **Given** the `TaskStatus` enum in `crates/domain/src/model/task.rs`
   **When** I inspect its definition
   **Then** it has the `#[non_exhaustive]` attribute
   **And** within the `domain` crate, all match statements use explicit variant arms (no wildcard)
   **And** in other crates (`application`, `infrastructure`, `api`), match statements use explicit arms for all known variants plus a `_ => return Err(...)` or `_ => unreachable!("unknown TaskStatus variant")` arm to satisfy the non-exhaustive requirement — never a silent fallthrough

3. **`delete_task` explicit match arms (CR13)**

   **Given** the `delete_task` function in the HTTP handler
   **When** I inspect its match on task status
   **Then** the catch-all `_ =>` arm is replaced with explicit arms: `Pending` (should not reach this branch — CTE handles it), `Running`, `Completed`, `Failed`, `Cancelled`, plus a `_ =>` arm returning HTTP 500 for future unknown variants
   **And** each arm produces an appropriate HTTP status code and error message

4. **Combined verification**

   **Given** the combined changes
   **When** `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D clippy::pedantic` run
   **Then** all tests pass and no new clippy warnings are introduced
   **And** existing cancel-related integration tests still pass
   **And** `cargo sqlx prepare --check --workspace` passes (cancel CTE changes the cached query)

## Tasks / Subtasks

- [x] **Task 1: Replace cancel SQL with atomic CTE** (AC: 1)
  - [x] 1.1: In `crates/infrastructure/src/adapters/postgres_task_repository.rs` (lines 611–650), replace the two-step cancel implementation (UPDATE + fallback SELECT) with a single CTE query
  - [x] 1.2: The CTE must atomically: attempt `UPDATE ... SET status = 'cancelled' WHERE id = $1 AND status = 'pending'`, then return the result alongside a fallback `SELECT status FROM tasks WHERE id = $1` in a single round-trip
  - [x] 1.3: Parse the CTE result to produce `CancelResult::Cancelled`, `CancelResult::NotFound`, or `CancelResult::NotCancellable` without a TOCTOU window
  - [x] 1.4: Run `cargo sqlx prepare --workspace` to regenerate `.sqlx/` cache
  - [x] 1.5: Verify `cargo sqlx prepare --check --workspace` passes

- [x] **Task 2: Add `#[non_exhaustive]` to TaskStatus** (AC: 2)
  - [x] 2.1: Add `#[non_exhaustive]` attribute to `TaskStatus` enum at `crates/domain/src/model/task.rs:61`
  - [x] 2.2: Verify domain-crate internal matches use explicit arms (no wildcard needed within the defining crate)
  - [x] 2.3: Update all match-on-TaskStatus sites in external crates to add `_ =>` wildcard arm with appropriate error handling (see Dev Notes for complete list)
  - [x] 2.4: Compile all crates to verify no exhaustiveness errors

- [x] **Task 3: Replace `delete_task` catch-all with explicit arms** (AC: 3)
  - [x] 3.1: In `crates/api/src/http/handlers/tasks.rs` (lines 196–204), replace the `_ =>` arm with explicit arms for `Pending`, `Running`, `Completed`, `Failed`, `Cancelled`
  - [x] 3.2: Add a `_ =>` arm at the end that returns HTTP 500 with an internal error message for unknown future variants
  - [x] 3.3: `Pending` arm: should be unreachable after CTE change — return HTTP 500 with a diagnostic message (the CTE guarantees pending tasks are cancelled atomically)
  - [x] 3.4: `Running` arm: HTTP 409 with `TASK_ALREADY_CLAIMED` (existing behavior)
  - [x] 3.5: `Completed`, `Failed`, `Cancelled` arms: HTTP 409 with `TASK_IN_TERMINAL_STATE` (existing behavior, now explicit)

- [x] **Task 4: Update all remaining TaskStatus match sites** (AC: 2)
  - [x] 4.1: `status_to_str` in `crates/infrastructure/src/adapters/postgres_task_repository.rs:159–167` — add `_ =>` wildcard returning an error or logging unknown variant
  - [x] 4.2: `status_to_str` in `crates/api/src/http/handlers/tasks.rs:92–101` — add `_ =>` wildcard
  - [x] 4.3: `parse_status_filter` in `crates/api/src/http/handlers/tasks.rs:232–243` — already has `other =>` error; verify it compiles with `#[non_exhaustive]`
  - [x] 4.4: `format_status` in `crates/api/src/cli/output.rs:15–23` — add `_ =>` wildcard
  - [x] 4.5: `parse_status` in `crates/api/src/cli/tasks.rs:27–38` — already has `other =>` error; verify it compiles
  - [x] 4.6: Check for any other TaskStatus matches added since Story 6.2 (grep for `TaskStatus::`)

- [x] **Task 5: Verify all tests pass** (AC: 4)
  - [x] 5.1: `cargo test --workspace` — all tests pass
  - [x] 5.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings
  - [x] 5.3: `cargo fmt --check` — clean
  - [x] 5.4: `cargo sqlx prepare --check --workspace` — passes
  - [x] 5.5: Specifically verify all 6 cancel tests in `rest_api_test.rs` pass (cancel_pending, cancel_nonexistent, cancel_already_cancelled, concurrent_cancel, cancel_running, cancel_completed)

### Senior Developer Review (AI)

**Review Date:** 2026-04-23
**Review Outcome:** Approve
**Reviewer Model:** Claude Opus 4.6 (1M context)
**Review Layers:** Blind Hunter, Edge Case Hunter, Acceptance Auditor

#### Action Items

- [x] [Review][Patch] Sanitize `AppError::internal` error messages in `delete_task` handler to avoid leaking CTE implementation details to API consumers [`crates/api/src/http/handlers/tasks.rs:197`] — The message "cancel CTE invariant violation: pending task was not cancelled" exposes SQL internals. Follow the existing pattern in `From<TaskError> for AppError`: log detailed message server-side with `tracing::error!`, return generic "internal server error" to the client. The second `AppError::internal` call ("unrecognized state") is generic enough and does not need changing.

### Review Follow-ups (AI)

- [x] [AI-Review][Med] Sanitize internal error messages in `delete_task` HTTP 500 responses [`crates/api/src/http/handlers/tasks.rs:197`]

## Dev Notes

### Architecture Compliance

- **Crate boundaries (architecture lines 924–937):** `domain` crate has no dependencies on other workspace crates. The `#[non_exhaustive]` attribute affects all downstream crates.
- **SQL query verification (architecture lines 805–806):** All queries are compile-time verified via `.sqlx/` offline cache. The CTE change MUST be followed by `cargo sqlx prepare --workspace`.
- **Error conversion (architecture lines 702–710):** Never discard error context. The CTE result parsing must produce the same `CancelResult` variants as the current implementation.
- **Enforcement guidelines (architecture lines 758–780):** Never use `unwrap()` in production code; `expect("invariant: ...")` only where documented.

### Critical Implementation Guidance

**Cancel SQL CTE pattern:**

The current implementation at `crates/infrastructure/src/adapters/postgres_task_repository.rs:611–650` uses two sequential queries:
1. `UPDATE tasks SET status = 'cancelled' WHERE id = $1 AND status = 'pending' RETURNING *`
2. If no rows returned: `SELECT status FROM tasks WHERE id = $1` (disambiguate not-found vs not-cancellable)

The TOCTOU race (documented in `deferred-work.md` line 95): between the failed UPDATE and the disambiguating SELECT, a concurrent request could change the task status, causing a stale error reason. Worst case is wrong 409 error code.

**Recommended CTE approach:**

```sql
WITH cancel_attempt AS (
    UPDATE tasks
    SET status = 'cancelled', updated_at = now()
    WHERE id = $1 AND status = 'pending'
    RETURNING id, queue, kind, payload, status, priority,
              attempts, max_attempts, last_error,
              scheduled_at, claimed_by, claimed_until,
              created_at, updated_at
),
current_state AS (
    SELECT status FROM tasks WHERE id = $1
)
SELECT
    ca.id, ca.queue, ca.kind, ca.payload, ca.status, ca.priority,
    ca.attempts, ca.max_attempts, ca.last_error,
    ca.scheduled_at, ca.claimed_by, ca.claimed_until,
    ca.created_at, ca.updated_at,
    cs.status AS current_status
FROM cancel_attempt ca
FULL OUTER JOIN current_state cs ON TRUE
```

Result interpretation:
- If `ca.id IS NOT NULL`: cancellation succeeded → `CancelResult::Cancelled(record)`
- If `ca.id IS NULL AND cs.status IS NOT NULL`: task exists but not pending → `CancelResult::NotCancellable { current_status: parse_status(cs.status) }`
- If `ca.id IS NULL AND cs.status IS NULL`: task not found → `CancelResult::NotFound`

**Important:** This CTE runs as a single statement — Postgres guarantees atomicity within a single statement. No explicit `BEGIN/COMMIT` needed. The `cancel_attempt` CTE and `current_state` CTE see the same snapshot, eliminating the TOCTOU window.

**Postgres CTE visibility rule:** In a data-modifying CTE, sibling CTEs see the PRE-modification snapshot. This means `current_state` reads the original status (`'pending'`) even when `cancel_attempt` succeeds. Result interpretation MUST use `ca.id IS NOT NULL` as the authoritative success signal — NOT `cs.status`. When the cancel succeeds, `cs.status` will be `'pending'` (pre-update), not `'cancelled'`.

**Alternative simpler approach (if the FULL OUTER JOIN is awkward with sqlx):**

Use a single CTE that always returns a row:
```sql
WITH cancel_attempt AS (
    UPDATE tasks
    SET status = 'cancelled', updated_at = now()
    WHERE id = $1 AND status = 'pending'
    RETURNING *
)
SELECT
    ca.*,
    t.status AS original_status,
    (ca.id IS NOT NULL) AS was_cancelled,
    (t.id IS NOT NULL) AS task_exists
FROM (SELECT $1::uuid AS lookup_id) params
LEFT JOIN cancel_attempt ca ON ca.id = params.lookup_id
LEFT JOIN tasks t ON t.id = params.lookup_id AND ca.id IS NULL
```

Choose whichever approach maps cleanly to `sqlx::query_as` or `sqlx::query`. The key requirement is: **one SQL statement, no TOCTOU window, same three outcomes**.

**sqlx query type considerations:**

The current cancel query uses `sqlx::query_as::<_, TaskRow>(...)`. The CTE returns a different shape (additional columns like `current_status`, `was_cancelled`). Options:
1. Use `sqlx::query(...)` with manual column extraction via `.get::<Option<Uuid>, _>("id")` etc.
2. Define a new `CancelRow` struct with `#[derive(sqlx::FromRow)]` that includes the extra columns.
3. Use `sqlx::query_as::<_, CancelRow>(...)` for clean mapping.

Option 2 or 3 is recommended. The `CancelRow` can be a `pub(crate)` struct in the adapter module.

**`#[non_exhaustive]` impact analysis — complete match site inventory:**

All match sites on `TaskStatus` across external crates (outside `domain`) need a `_ =>` wildcard arm after adding `#[non_exhaustive]`. Here is the complete list:

| File | Function | Lines | Current State | Action Required |
|------|----------|-------|---------------|-----------------|
| `infrastructure/.../postgres_task_repository.rs` | `parse_status` | 146–157 | Has `other =>` error arm on string input — NOT a match on TaskStatus enum | None — matches on `&str`, not `TaskStatus` |
| `infrastructure/.../postgres_task_repository.rs` | `status_to_str` | 159–167 | Exhaustive on 5 variants, no wildcard | Add `_ =>` arm (e.g., return `"unknown"` or panic with descriptive message) |
| `api/.../handlers/tasks.rs` | `status_to_str` | 92–101 | Exhaustive on 5 variants, no wildcard | Add `_ =>` arm |
| `api/.../handlers/tasks.rs` | `delete_task` inner match | 196–204 | Has `_ =>` catch-all (the bug) | Replace with explicit arms + `_ =>` HTTP 500 |
| `api/.../handlers/tasks.rs` | `parse_status_filter` | 232–243 | Matches on `&str`, not TaskStatus | None — matches on `&str` |
| `api/src/cli/output.rs` | `format_status` | 15–23 | Exhaustive on 5 variants, no wildcard | Add `_ =>` arm |
| `api/src/cli/tasks.rs` | `parse_status` | 27–38 | Matches on `&str`, not TaskStatus | None — matches on `&str` |

**Additional match site in application crate:**

| `application/.../worker.rs` | `emit_task_failed` (inner match) | 710–794 | Has `other =>` catch-all with `error!` log — already defensive | Verify compiles; no change needed — the catch-all is intentionally a defense-in-depth error log, not a silent fallthrough |

**Net action:** 3 functions need `_ =>` wildcard arms added (`status_to_str` x2, `format_status`), plus the `delete_task` handler rewrite. The `worker.rs:710` match already has an appropriate `other =>` catch-all. Functions that match on `&str` input do NOT need changes.

**Domain-internal matches:** Within `crates/domain/`, `#[non_exhaustive]` has no effect — the crate that defines the enum can still match exhaustively. Verify no domain-internal matches exist that would be affected. The `TaskStatus` is defined in `domain` but primarily consumed in `infrastructure` and `api`.

**Test `matches!` macros are unaffected:** Several test files use `matches!(t.status, TaskStatus::Completed | TaskStatus::Failed)` (e.g., `common/otel.rs:193`, `audit_trail_test.rs:163`). The `matches!` macro does NOT require exhaustiveness — it checks listed variants and returns `false` for unlisted ones. No changes needed for test `matches!` patterns.

**`status_to_str` wildcard strategy:**

For `status_to_str` functions, the wildcard arm should NOT silently return a default string. Options:
1. **Recommended:** Return the variant's `Debug` representation: `_ => { let s = format!("{status:?}"); /* log warning */ s }` — but this requires changing the return type from `&'static str` to `String` (or `Cow<'static, str>`).
2. **Simpler:** Use `unreachable!("unknown TaskStatus variant: {status:?}")` — acceptable because adding a new variant to TaskStatus requires updating these functions simultaneously (it's the same crate author).
3. **Safest for the API handler:** Return a generic string like `"unknown"` and log a warning.

Since this is an internal project (not a published library consumed by third parties), option 2 (`unreachable!`) is acceptable for `status_to_str` functions. The `#[non_exhaustive]` attribute is primarily a semver safety net for future development, not defense against unknown runtime values.

For the `delete_task` handler, the `_ =>` arm MUST return HTTP 500 (not `unreachable!`) because it's a request handler that should never panic.

### Previous Story Intelligence

**From Story 6.2 (completed):**
- `concurrent_cancel_exactly_one_succeeds` test (rest_api_test.rs:607–671) fires 10 concurrent DELETE requests — exactly one gets 200, nine get 409. This test will continue to work after the CTE change and validates the atomicity improvement.
- `cancel_already_cancelled_task_returns_409` (rest_api_test.rs:561–601) double-cancels — verifies idempotency.
- Clippy pedantic is now enforced workspace-wide; all pre-existing violations were fixed in 6.2.
- `shutdown_test.rs` had a compilation error fixed in 6.2 (config moved before builder assertion).
- The `format!` in `status_to_str` at `tasks.rs:92-101` returns `String` (not `&'static str`) — the `to_string()` call is on a `&str` match arm. The wildcard arm can follow the same pattern.

**From Story 6.1 (completed):**
- `Notify`-based signalling pattern established as standard for deterministic test sync.
- All 47 application tests pass; compilation clean.

### Git Intelligence

Recent commits are planning/sprint docs (7ed6fc8, b346296, a5a03e6). Last code commit: `9e8fea5` (Story 5.3). Stories 6.1 and 6.2 are in commit `7ed6fc8` (stabilize timing-dependent tests).

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `TaskStatus` enum | `crates/domain/src/model/task.rs:61–67` | AC 2 — add `#[non_exhaustive]` |
| `CancelResult` enum | `crates/domain/src/model/task.rs:144–153` | AC 1 — return type from cancel |
| `cancel()` repo impl | `crates/infrastructure/src/adapters/postgres_task_repository.rs:611–650` | AC 1 — replace with CTE |
| `parse_status()` | `crates/infrastructure/src/adapters/postgres_task_repository.rs:146–157` | AC 2 — matches `&str`, no change needed |
| `status_to_str()` (infra) | `crates/infrastructure/src/adapters/postgres_task_repository.rs:159–167` | AC 2 — add `_ =>` arm |
| `delete_task()` handler | `crates/api/src/http/handlers/tasks.rs:186–206` | AC 3 — rewrite match |
| `status_to_str()` (api) | `crates/api/src/http/handlers/tasks.rs:92–101` | AC 2 — add `_ =>` arm |
| `parse_status_filter()` | `crates/api/src/http/handlers/tasks.rs:232–243` | Matches `&str`, no change needed |
| `format_status()` | `crates/api/src/cli/output.rs:15–23` | AC 2 — add `_ =>` arm |
| `parse_status()` (cli) | `crates/api/src/cli/tasks.rs:27–38` | Matches `&str`, no change needed |
| `TaskRepository::cancel()` trait | `crates/application/src/ports/task_repository.rs:82–86` | AC 1 — trait signature unchanged |
| `SchedulerService::cancel()` | `crates/application/src/services/scheduler.rs:169–177` | Pass-through, unchanged |
| `IronDefer::cancel()` | `crates/api/src/lib.rs:313` | Public API, unchanged |
| `.sqlx/` cache | `.sqlx/query-*.json` at workspace root | AC 4 — must regenerate after CTE |
| `MockTaskRepository` | `crates/application/src/ports/task_repository.rs` (mockall) | AC 2 — mock auto-generated, verify cancel expectations still compile |

### Existing Cancel Test Inventory

| Test | File | Lines | What It Verifies |
|------|------|-------|------------------|
| `cancel_pending_task_returns_200` | `crates/api/tests/rest_api_test.rs` | 486–535 | Happy path cancel |
| `cancel_nonexistent_task_returns_404` | `crates/api/tests/rest_api_test.rs` | 539–557 | 404 for unknown UUID |
| `cancel_already_cancelled_task_returns_409` | `crates/api/tests/rest_api_test.rs` | 561–601 | Double-cancel idempotency |
| `concurrent_cancel_exactly_one_succeeds` | `crates/api/tests/rest_api_test.rs` | 607–671 | 10 concurrent DELETEs — exactly 1 wins |
| `cancel_running_task_returns_409` | `crates/api/tests/rest_api_test.rs` | 675–749 | Can't cancel running task |
| `cancel_completed_task_returns_409` | `crates/api/tests/rest_api_test.rs` | 753–830 | Can't cancel completed task |
| `cancel_delegates_to_repo` | `crates/application/src/services/scheduler.rs` | 360–375 | Unit: SchedulerService delegates |
| `cancel_returns_not_found` | `crates/application/src/services/scheduler.rs` | 377–390 | Unit: MockRepo returns NotFound |

All 8 tests must continue to pass after changes. No new tests required — existing coverage is comprehensive.

### Dependencies

No new crate dependencies. All changes are to existing code:
- `sqlx` — CTE query (already in workspace)
- `serde` — no changes to serialization
- No schema migration needed — the CTE uses the same table, same columns

### Project Structure Notes

- **Modified files only** — no new files created
- `crates/domain/src/model/task.rs` — add `#[non_exhaustive]` attribute
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — CTE cancel query + `status_to_str` wildcard
- `crates/api/src/http/handlers/tasks.rs` — `delete_task` rewrite + `status_to_str` wildcard
- `crates/api/src/cli/output.rs` — `format_status` wildcard
- `.sqlx/` — regenerated cache files (auto-generated by `cargo sqlx prepare`)

### Out of Scope

- **Sweeper atomicity** — Story 6.4 scope (CR8)
- **Error model restructuring** (`InvalidPayload`/`ExecutionFailed` structured types) — Story 6.6 scope (CR10, CR11)
- **Error payload scrubbing** — Story 6.7 scope (CR14)
- **`list_tasks` COUNT/SELECT race** — Story 6.8 scope (CR2)
- **Field visibility / accessor methods** — Story 6.10 scope (CR46)
- **`CancelResult` `#[non_exhaustive]`** — not in AC scope; `CancelResult` is domain-internal and only constructed/matched within the same codebase

### References

- [Source: `docs/artifacts/planning/epics.md` lines 312–339] — Story 6.3 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 270–298] — Tasks table schema and cancel semantics
- [Source: `docs/artifacts/planning/architecture.md` lines 805–806] — SQLx compile-time verification requirement
- [Source: `docs/artifacts/planning/architecture.md` lines 758–780] — Enforcement guidelines
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 95–103] — CR1 (TOCTOU), CR12 (#[non_exhaustive]), CR13 (catch-all) deferred items
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs:611–650`] — Current cancel implementation
- [Source: `crates/domain/src/model/task.rs:61–67`] — TaskStatus enum definition
- [Source: `crates/api/src/http/handlers/tasks.rs:186–206`] — delete_task handler with `_ =>` catch-all
- [Source: `crates/api/tests/rest_api_test.rs:486–830`] — All 6 cancel integration tests

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

- Task 1: Replaced two-step cancel (UPDATE + SELECT) with atomic CTE using `WITH cancel_attempt AS (UPDATE ... RETURNING *) SELECT ... LEFT JOIN cancel_attempt ... LEFT JOIN tasks ...`. Eliminates TOCTOU race between status check and update. Defined local `CancelRow` struct with `#[derive(sqlx::FromRow)]` for CTE result mapping. Uses `was_cancelled` boolean and `task_exists` boolean for clean result interpretation.
- Task 2: Added `#[non_exhaustive]` to `TaskStatus` enum. Verified all domain-internal matches compile without wildcard (domain crate defines the enum so exhaustive matching is still allowed). Added `_ => unreachable!(...)` arms to 3 external functions: `status_to_str` (infra), `status_to_str` (api), `format_status` (cli). Verified `worker.rs:710` already has appropriate `other =>` catch-all. Verified `parse_status_filter` and `parse_status` (cli) match on `&str`, not `TaskStatus`, so no changes needed.
- Task 3: Replaced `delete_task` catch-all with explicit arms: `Pending` (HTTP 500 CTE invariant violation), `Running` (HTTP 409 TASK_ALREADY_CLAIMED), `Completed|Failed|Cancelled` (HTTP 409 TASK_IN_TERMINAL_STATE), `_` (HTTP 500 unrecognized state). Added `AppError::internal()` constructor for HTTP 500 responses.
- Task 4: All match sites verified. 3 functions updated with `_ => unreachable!()`, 2 string-matching functions confirmed unaffected, worker.rs catch-all confirmed adequate. Grep for `TaskStatus::` found no additional match sites requiring changes.
- Task 5: All 239 tests pass. All 6 cancel integration tests pass. Clippy pedantic clean (pre-existing warnings only). `cargo fmt --check` clean. `cargo sqlx prepare --check --workspace` passes.

### File List

- `crates/domain/src/model/task.rs` — added `#[non_exhaustive]` to `TaskStatus` enum
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — replaced cancel with atomic CTE; added `_ => unreachable!()` to `status_to_str`
- `crates/api/src/http/handlers/tasks.rs` — replaced `delete_task` catch-all with explicit arms; added `_ => unreachable!()` to `status_to_str`
- `crates/api/src/http/errors.rs` — added `AppError::internal()` constructor
- `crates/api/src/cli/output.rs` — added `_ => unreachable!()` to `format_status`
- `.sqlx/` — regenerated offline cache (9 query files)
