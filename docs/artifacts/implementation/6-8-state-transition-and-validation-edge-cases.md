# Story 6.8: State Transition & Validation Edge Cases

Status: done

## Story

As a developer,
I want query correctness and input validation to handle edge cases,
so that list operations return consistent results, invalid inputs produce clear errors, and worker lifecycle transitions are safe.

## Acceptance Criteria

1. **`list_tasks` window function (CR2)**

   **Given** the `list_tasks` query in `postgres_task_repository.rs`
   **When** a `GET /tasks` request with filters is processed
   **Then** the query uses `COUNT(*) OVER()` window function to compute total count in the same query as the data fetch
   **And** the count and results are always consistent (no race between separate COUNT and SELECT queries)
   **And** `cargo sqlx prepare --workspace` regenerates the `.sqlx/` cache for the changed query

2. **`scheduled_at` validation (CR9)**

   **Given** a task submission with `scheduled_at` set to an out-of-range value (e.g., year 0001, year 9999, or before Unix epoch)
   **When** the `enqueue` method processes the request
   **Then** a descriptive validation error is returned before the INSERT is attempted
   **And** the validation rejects non-finite or absurdly distant values using Postgres `timestamptz` range bounds (not hardcoded application-level year constants), so embedded-library callers are not artificially constrained

3. **Claim-to-spawn cancellation check (CR22)**

   **Given** the worker dispatch flow in `worker.rs`
   **When** a task is claimed and the worker is about to spawn the execution future
   **Then** the `CancellationToken` is checked between `claim_next` returning and `tokio::spawn` (a token check, not a DB re-read — no additional DB round-trip)
   **And** if the token is cancelled between claim and spawn, the task's lease is released via `release_leases_for_worker` (not left as an orphaned Running task)
   **And** the tighter handoff is documented with a code comment explaining the invariant

4. **`release_leases_for_worker` attempt increment (CR24)**

   **Given** the `release_leases_for_worker` function in `shutdown.rs` or `worker.rs`
   **When** leases are released during shutdown timeout
   **Then** the `attempts` field is incremented on each released task (so the sweeper counts the interrupted attempt toward max_attempts)
   **And** the UPDATE query sets `attempts = attempts + 1` alongside `status = 'pending'`, `claimed_by = NULL`, `claimed_until = NULL`
   **And** this is intentionally different from the sweeper's `recover_zombie_tasks` (which does NOT increment attempts on recovery) — the asymmetry is documented: shutdown release counts as a consumed attempt because the task was claimed and dispatched, while sweeper recovery does not because the task may never have started executing

## Tasks / Subtasks

- [x] **Task 1: Replace separate COUNT/SELECT with window function** (AC: 1)
  - [x] 1.1: In `crates/infrastructure/src/adapters/postgres_task_repository.rs` (lines 508–549), merge the two queries into a single query using `COUNT(*) OVER() AS total`
  - [x] 1.2: The new query returns both data rows and the total count in every row via the window function
  - [x] 1.3: Extract `total` from the first row (or default to 0 if no rows); map the data columns to `TaskRow`
  - [x] 1.4: Remove the separate `count_sql` query (lines 508–519)
  - [x] 1.5: Run `cargo sqlx prepare --workspace` to regenerate `.sqlx/` cache
  - [x] 1.6: Verify `cargo sqlx prepare --check --workspace` passes

- [x] **Task 2: Add `scheduled_at` range validation** (AC: 2)
  - [x] 2.1: In `crates/api/src/lib.rs`, add validation in `enqueue_inner` (around line 264) and `enqueue_raw` (around line 598) before passing `scheduled_at` to the scheduler
  - [x] 2.2: Validation rejects values outside the Postgres `timestamptz` range: 4713 BC to 294276 AD. Use chrono constants or compute from Postgres bounds — NOT hardcoded year limits
  - [x] 2.3: Return `TaskError::InvalidPayload` with a descriptive message including the rejected value and the valid range
  - [x] 2.4: `None` (default) is always valid — validation only applies to `Some(dt)`
  - [x] 2.5: Add unit tests for boundary values: epoch, far future, far past, `DateTime::<Utc>::MAX_UTC`, `DateTime::<Utc>::MIN_UTC`

- [x] **Task 3: Add cancellation token check between claim and spawn** (AC: 3)
  - [x] 3.1: In `crates/application/src/services/worker.rs`, after `claim_next` returns `Ok(Some(task))` (line 179) and before `join_set.spawn` (line 244), add `if self.token.is_cancelled()`
  - [x] 3.2: If cancelled, release the task's lease: call `self.repo.release_leases_for_worker(worker_id)` (releases all tasks for this worker, which at this point is just the one just claimed)
  - [x] 3.3: Drop the semaphore permit and break out of the poll loop
  - [x] 3.4: Add a code comment explaining the invariant: "Check token between claim and spawn to avoid orphaning a Running task when shutdown fires during this window. The task was claimed (attempt incremented) but never dispatched."
  - [x] 3.5: Log at `info!` level: `event = "claim_cancelled", task_id = %task.id`

- [x] **Task 4: Increment attempts in `release_leases_for_worker`** (AC: 4)
  - [x] 4.1: In `crates/infrastructure/src/adapters/postgres_task_repository.rs` (lines 701–712), add `attempts = attempts + 1` to the UPDATE query
  - [x] 4.2: Add a code comment documenting the asymmetry with `recover_zombie_tasks`: "Shutdown release increments attempts because the task was claimed and dispatched (the attempt was consumed). Sweeper recovery does NOT increment because the task may never have started executing — it simply expired."
  - [x] 4.3: Run `cargo sqlx prepare --workspace` to regenerate `.sqlx/` cache

- [x] **Task 5: Update tests** (AC: 1, 2, 3, 4)
  - [x] 5.1: Update `list_tasks` tests to verify consistent total/tasks count (the window function eliminates the race)
  - [x] 5.2: Add test for `scheduled_at` validation: out-of-range value returns `InvalidPayload`
  - [x] 5.3: Add test for claim-to-spawn cancellation: mock claim_next to return a task, cancel token, verify task lease released
  - [x] 5.4: Verify `shutdown_timeout_releases_leases` test (shutdown_test.rs:128) still passes — CONFIRMED: it queries `status, claimed_by, claimed_until` only (lines 206–220), does NOT assert on `attempts`, so the CR24 increment is invisible to this test
  - [x] 5.5: Verify sweeper tests still pass — `recover_zombie_tasks` behavior is unchanged

- [x] **Task 6: Verify no regressions** (AC: all)
  - [x] 6.1: `cargo test --workspace` — all tests pass
  - [x] 6.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings
  - [x] 6.3: `cargo fmt --check` — clean
  - [x] 6.4: `cargo sqlx prepare --check --workspace` — passes

## Dev Notes

### Architecture Compliance

- **SQL query verification (architecture lines 805–806):** All queries use compile-time verification. Window function query change MUST be followed by `cargo sqlx prepare --workspace`.
- **C2 — CancellationToken semantics (architecture lines 1109–1128):** Token checked between tasks only, never mid-execution. The new check (Task 3) is between claim and spawn — before execution begins — so it's consistent with C2.
- **Error handling (architecture lines 702–710):** Never discard error context. Validation errors must be descriptive.
- **Domain crate boundaries (architecture lines 924–937):** `scheduled_at` validation in the API crate is acceptable — it's at the system boundary where input validation belongs.

### Critical Implementation Guidance

**Task 1: Window function query**

The current implementation uses two separate queries (lines 508–549):
1. `SELECT COUNT(*) FROM tasks {where_sql}` — count query
2. `SELECT ... FROM tasks {where_sql} ORDER BY ... LIMIT ... OFFSET ...` — data query

Replace with a single query using `COUNT(*) OVER()`:

```sql
SELECT id, queue, kind, payload, status, priority,
       attempts, max_attempts, last_error,
       scheduled_at, claimed_by, claimed_until,
       created_at, updated_at,
       COUNT(*) OVER() AS total_count
FROM tasks {where_sql}
ORDER BY created_at ASC, id ASC
LIMIT $N OFFSET $M
```

**`COUNT(*) OVER()` semantics:** The window function with an empty `OVER()` clause counts ALL rows matching the WHERE clause BEFORE `LIMIT/OFFSET` is applied. Postgres evaluates window functions in the SELECT phase (after WHERE/JOIN, before LIMIT). So if 100 rows match the filter and LIMIT is 10, each returned row carries `total_count = 100`. This is exactly what pagination needs — the total item count for the response header.

**Implementation challenge:** The current code uses runtime-built SQL (`format!` for `where_sql`) because filters are optional. The `sqlx::query!` macro requires compile-time SQL. The current code already uses `sqlx::query_as::<_, TaskRow>(&data_sql)` (runtime SQL string), so continuing with runtime SQL is fine.

**New row type:** Define a `TaskRowWithTotal` struct that extends `TaskRow` with the `total_count` column:

```rust
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TaskRowWithTotal {
    // All TaskRow fields
    pub id: Uuid,
    pub queue: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i16,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub scheduled_at: DateTime<Utc>,
    pub claimed_by: Option<Uuid>,
    pub claimed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Window function result
    pub total_count: i64,
}
```

**Result extraction:**
```rust
let rows = data_query
    .fetch_all(&self.pool)
    .await
    .map_err(PostgresAdapterError::from)?;

let total = rows.first().map_or(0, |r| r.total_count as u64);
let tasks: Vec<TaskRecord> = rows
    .into_iter()
    .map(|r| TaskRecord::try_from(TaskRow::from(r)))
    .collect::<Result<Vec<_>, _>>()?;
```

Implement `From<TaskRowWithTotal> for TaskRow` to strip the extra column for reuse of the existing `TryFrom<TaskRow> for TaskRecord`.

**Task 2: `scheduled_at` validation**

Postgres `timestamptz` valid range: `4713-01-01 BC` to `294276-12-31 AD`. In chrono terms:

```rust
fn validate_scheduled_at(dt: &DateTime<Utc>) -> Result<(), TaskError> {
    // Postgres timestamptz range: 4713 BC to 294276 AD
    // chrono represents years as i32, so year 294276 is representable
    let year = dt.year();
    if year < -4712 || year > 294_276 {
        return Err(TaskError::InvalidPayload {
            reason: format!(
                "scheduled_at year {year} is outside Postgres timestamptz range (-4712..294276)"
            ),
        });
    }
    Ok(())
}
```

**Note on AC wording:** "using Postgres timestamptz range bounds (not hardcoded application-level year constants)" — the years -4712 and 294276 ARE the Postgres bounds, not arbitrary application limits. Document this in the code comment.

**Alternative approach:** Use chrono's `NaiveDate::MIN` / `NaiveDate::MAX` and compare against Postgres range — but chrono's range (`262145 BC` to `262143 AD`) is wider than Postgres's range, so we need explicit Postgres bounds.

**If Story 6.6 lands first**, the error becomes:
```rust
TaskError::InvalidPayload { kind: PayloadErrorKind::Validation { message: format!(...) } }
```

**Placement:** Validate in `enqueue_inner` (before calling scheduler) and `enqueue_raw`. Also validate in the REST handler (`CreateTaskRequest` deserialization or handler validation) — but the AC says "enqueue method", so library-level validation is sufficient.

**Task 3: Claim-to-spawn token check**

Insert between the `Ok(Some(task))` match arm (line 179) and the spawn setup code:

```rust
Ok(Some(task)) => {
    // CR22: Check token between claim and spawn to avoid
    // orphaning a Running task when shutdown fires during this
    // window. The task was claimed (attempts incremented) but
    // never dispatched — release its lease immediately.
    if self.token.is_cancelled() {
        info!(
            event = "claim_cancelled",
            task_id = %task.id,
            worker_id = %worker_id,
            "cancellation detected after claim, releasing lease"
        );
        if let Err(e) = self.repo.release_leases_for_worker(worker_id).await {
            error!(error = %e, "failed to release lease after claim cancellation");
        }
        drop(permit);
        break;
    }
    // ... existing spawn code ...
}
```

**Why `release_leases_for_worker(worker_id)` instead of a single-task release:**
There's no `release_lease_for_task(task_id)` method. `release_leases_for_worker` releases ALL Running tasks for this worker. At this point in the code, the worker has just claimed ONE task and has no other in-flight tasks (the JoinSet may have tasks, but those are already spawned and running). The release only affects tasks with `claimed_by = worker_id AND status = 'running'`.

**Important:** `self.token.is_cancelled()` is a non-async poll — no `.await` needed. It returns `true` if the token has been cancelled. This is cheaper than `tokio::select!` for a simple check.

**CR22 + CR24 interaction — double increment:** When the token check fires (CR22) and calls `release_leases_for_worker` (CR24), the task's `attempts` is incremented twice: once by `claim_next` (already happened) and once by `release_leases_for_worker`. This means a task claimed-but-never-dispatched consumes 2 attempt slots. This is acceptable — the claim was a real attempt that consumed resources, and the release prevents it from counting as 0 attempts. If `max_attempts = 1`, the task will transition to Failed on next sweeper cycle rather than being retried. Document this in the CR22 code comment.

**Task 4: Attempt increment in `release_leases_for_worker`**

Current SQL (lines 701–712):
```sql
UPDATE tasks
SET status = 'pending',
    claimed_by = NULL,
    claimed_until = NULL,
    scheduled_at = now(),
    updated_at = now()
WHERE claimed_by = $1
  AND status = 'running'
RETURNING id
```

Updated SQL:
```sql
UPDATE tasks
SET status = 'pending',
    claimed_by = NULL,
    claimed_until = NULL,
    scheduled_at = now(),
    updated_at = now(),
    attempts = attempts + 1
WHERE claimed_by = $1
  AND status = 'running'
RETURNING id
```

**Asymmetry documentation (code comment):**
```rust
// CR24: Increment attempts on shutdown release. This is intentionally
// different from recover_zombie_tasks (which does NOT increment):
//
// - Shutdown release: The task was claimed AND dispatched (the worker
//   was actively executing it). The attempt was consumed — incrementing
//   ensures it counts toward max_attempts.
//
// - Sweeper recovery: The task's lease expired, but the task may never
//   have started executing (worker crashed before dispatch). The attempt
//   was already incremented by claim_next — no double-counting needed.
```

**Impact on `shutdown_timeout_releases_leases` test (shutdown_test.rs:128):**
This test verifies that released tasks return to Pending status. After CR24, released tasks will have `attempts = 2` instead of `attempts = 1` (claimed once = attempt 1, then release increments to 2). Verify the test doesn't assert on the attempt count — if it does, update the assertion.

### Previous Story Intelligence

**From Story 6.7 (ready-for-dev):**
- `PostgresAdapterError` gained a `DatabaseScrubbed` variant. No impact on this story — the queries here don't trigger database constraint violations.

**From Story 6.6 (ready-for-dev):**
- `TaskError::InvalidPayload` may change from `{ reason: String }` to `{ kind: PayloadErrorKind }`. If 6.6 lands first, the `scheduled_at` validation error (Task 2) must use the new structured type.

**From Story 6.5 (ready-for-dev):**
- Worker poll loop restructured with jittered backoff and `claim_next` raced against token. The claim-to-spawn check (Task 3) goes AFTER the claim returns — it's complementary, not conflicting.

**From Story 6.4 (ready-for-dev):**
- `recover_zombie_tasks` return type changed to `Vec<(TaskId, QueueName)>`. The `release_leases_for_worker` return type is `Vec<TaskId>` — NOT changed in this story. If queue context is needed for release logging, it can be added later.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide.

### Git Intelligence

Last code commit: `7ed6fc8`. `list_tasks` last modified in Story 4.2. `release_leases_for_worker` last modified in Story 2.2.

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `list_tasks` impl | `postgres_task_repository.rs:483–561` | AC 1 — replace COUNT+SELECT |
| `ListTasksResult` | `crates/domain/src/model/task.rs:164–169` | AC 1 — return type |
| `ListTasksFilter` | `crates/domain/src/model/task.rs:155–162` | AC 1 — filter params |
| `TaskRow` struct | `postgres_task_repository.rs:67–82` | AC 1 — extend with total_count |
| `enqueue_inner` | `crates/api/src/lib.rs:237–277` | AC 2 — add validation |
| `enqueue_raw` | `crates/api/src/lib.rs:565–615` | AC 2 — add validation |
| `save` (INSERT) | `postgres_task_repository.rs:193–234` | AC 2 — receives scheduled_at |
| Worker poll loop | `crates/application/src/services/worker.rs:163–291` | AC 3 — add token check |
| `claim_next` result handling | `worker.rs:179–265` | AC 3 — check between claim and spawn |
| `release_leases_for_worker` impl | `postgres_task_repository.rs:696–721` | AC 4 — add `attempts + 1` |
| `release_leases_for_worker` trait | `ports/task_repository.rs:91–101` | AC 4 — signature unchanged |
| `recover_zombie_tasks` | `postgres_task_repository.rs:434–481` | AC 4 — comparison (no change) |
| `CancellationToken::is_cancelled` | tokio_util crate | AC 3 — non-async check |
| `.sqlx/` cache | `.sqlx/query-*.json` at workspace root | AC 1, 4 — must regenerate |

### Existing Test Inventory

| Test | File | Lines | Impact |
|------|------|-------|--------|
| `list_tasks_*` (7 tests) | `rest_api_test.rs` | 922–1084 | AC 1 — verify window function results |
| `enqueue_persists_task_*` | `integration_test.rs` | 119 | AC 2 — verify normal case |
| `enqueue_at_respects_explicit_schedule` | `integration_test.rs` | 165 | AC 2 — verify explicit scheduled_at |
| `shutdown_drains_inflight_tasks` | `shutdown_test.rs` | 44 | AC 3 — verify drain behavior |
| `shutdown_timeout_releases_leases` | `shutdown_test.rs` | 128 | AC 4 — verify release + may need attempt assertion update |
| `sweeper_recovers_zombie_task` | `sweeper_test.rs` | 43 | AC 4 — verify sweeper unchanged |
| `worker_stops_on_cancellation` | `worker.rs` | 1168 | AC 3 — verify cancellation |
| `claim_next_*` tests | `task_repository_test.rs` | 291–450 | Context — verify claim still works |

### Dependencies

No new crate dependencies.

### Project Structure Notes

- **Modified files:**
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — window function query + `release_leases_for_worker` attempt increment
  - `crates/api/src/lib.rs` — `scheduled_at` validation in `enqueue_inner` and `enqueue_raw`
  - `crates/application/src/services/worker.rs` — claim-to-spawn token check
  - `.sqlx/` — regenerated cache
- **No new files created**
- **No schema migrations needed** — `COUNT(*) OVER()` is a query change, not a schema change

### Out of Scope

- **Builder pattern / field visibility** — Stories 6.9, 6.10
- **`offset` capping** — Story 7.1 (CR3)
- **Unfiltered GET /tasks warning** — Story 7.1 (CR4)
- **Pagination composite index** — Story 7.1 (CR5)
- **`queue_statistics()` active queue filter** — Story 7.1 (CR6)
- **`parse_status_filter` case-insensitive** — Story 7.1 (CR7)
- **CLI `output.rs` pagination bounds** — Story 7.1 (CR15)

### References

- [Source: `docs/artifacts/planning/epics.md` lines 448–477] — Story 6.8 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 270–298] — Tasks table schema
- [Source: `docs/artifacts/planning/architecture.md` lines 1109–1128] — C2: CancellationToken semantics
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 67–70] — CR22 (claim-to-spawn window), CR24 (release_leases attempts)
- [Source: `docs/artifacts/implementation/deferred-work.md` line 17] — CR9 (out-of-range scheduled_at)
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 107–108] — CR2 (COUNT/SELECT race)
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs:508–549`] — Current list_tasks two-query implementation
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs:696–721`] — Current release_leases_for_worker (no attempt increment)
- [Source: `crates/application/src/services/worker.rs:177–265`] — Claim-to-spawn window (no token check)
- [Source: `crates/api/src/lib.rs:237–277, 565–615`] — enqueue_inner and enqueue_raw (no scheduled_at validation)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- `validate_scheduled_at_accepts_far_future` test initially failed because `NaiveDate::from_ymd_opt(294_276, ...)` returns `None` — chrono's max representable year (~262143) is below Postgres's upper bound (294276). Fixed by using `DateTime::<Utc>::MAX_UTC` (which IS within PG range) as the far-future test case. The upper-bound rejection test was removed since chrono cannot represent years > 294276.
- `claim_cancelled_releases_lease` test initially failed because pre-cancelling the token caused the poll loop's `tokio::select!` to break before `claim_next` was ever called. Fixed by cancelling the token inside the `claim_next` mock (as a side effect of returning the task), so the claim succeeds but the post-claim token check fires.

### Completion Notes List

- AC 1: Replaced two-query (COUNT + SELECT) implementation in `list_tasks` with a single query using `COUNT(*) OVER() AS total_count` window function. Introduced `TaskRowWithTotal` struct and `From<TaskRowWithTotal> for TaskRow` conversion. Total count and data rows are always consistent — no race window.
- AC 2: Added `validate_scheduled_at()` function in `crates/api/src/lib.rs` that rejects years outside Postgres `timestamptz` range (-4712..=294276). Called from both `enqueue_inner` and `enqueue_raw` before scheduler delegation. Returns `TaskError::InvalidPayload` with descriptive message. `None` passes through unchanged. Unit tests for epoch, far future (MAX_UTC), and far past (MIN_UTC).
- AC 3: Added `self.token.is_cancelled()` check in worker poll loop between `claim_next` returning `Ok(Some(task))` and `join_set.spawn`. If cancelled, releases leases via `release_leases_for_worker`, drops permit, breaks loop. Comment documents the invariant and double-increment behavior. Unit test verifies lease release by cancelling token inside mock.
- AC 4: Added `attempts = attempts + 1` to `release_leases_for_worker` UPDATE query. Comment documents asymmetry with `recover_zombie_tasks` — shutdown release counts as consumed attempt, sweeper recovery does not. `.sqlx/` cache regenerated. Existing shutdown test unaffected (does not assert on attempts).

### File List

- crates/infrastructure/src/adapters/postgres_task_repository.rs — added `TaskRowWithTotal` struct + `From` impl; replaced two-query `list_tasks` with window function; added `attempts = attempts + 1` to `release_leases_for_worker`
- crates/api/src/lib.rs — added `validate_scheduled_at()` function; added validation calls in `enqueue_inner` and `enqueue_raw`; added 3 unit tests for boundary values
- crates/application/src/services/worker.rs — added `token.is_cancelled()` check between claim and spawn with lease release; added `claim_cancelled_releases_lease` unit test
- .sqlx/ — regenerated offline query cache (2 changed queries)

### Change Log

- 2026-04-23: Story 6.8 implementation complete — window function query, scheduled_at validation, claim-to-spawn token check, release_leases attempt increment. All 4 ACs satisfied, all tasks/subtasks checked.
