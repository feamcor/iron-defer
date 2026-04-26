# Story 2.1: Sweeper — Zombie Task Recovery

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a platform engineer,
I want expired-lease tasks automatically recovered by a background sweeper,
So that worker crashes never cause permanent task loss.

## Acceptance Criteria

1. **`SweeperService` created as an independent background task** in `crates/application/src/services/sweeper.rs`:
   - Holds `Arc<dyn TaskRepository>`, `sweeper_interval: Duration`, and a child `CancellationToken` cloned from the root token.
   - Runs as an independent `tokio::spawn`'d task with its own `tokio::time::interval` (Architecture D3.1).
   - NOT embedded in the worker poll loop — separate concern, separate task.
   - Public method: `async fn run(&self) -> Result<(), TaskError>` — the sweep loop.

2. **Zombie recovery — retryable tasks** (FR13):
   - Given a task in `Running` status with `claimed_until` in the past and `attempts < max_attempts`,
   - The sweeper resets it to `Pending`, clears `claimed_by` and `claimed_until`, and sets `scheduled_at = now()`.
   - The task becomes eligible for re-claiming by any worker.

3. **Zombie recovery — exhausted tasks** (FR43):
   - Given a task in `Running` status with `claimed_until` in the past and `attempts >= max_attempts`,
   - The sweeper transitions it to `Failed` with `last_error = 'lease expired: max attempts exhausted'`.
   - The task is never re-queued as `Pending`.

4. **OTel metric emitted** (deferred to Epic 3 for actual OTel wiring — this story logs the count):
   - After each recovery cycle, the sweeper logs the count of recovered tasks at `info!` level, labeled by queue.
   - The method returns the recovery count so Epic 3 can wire `iron_defer_zombie_recoveries_total` counter later.

5. **`WorkerConfig` extended with `sweeper_interval`**:
   - New field: `pub sweeper_interval: Duration` with default `Duration::from_secs(60)` (FR15).
   - `#[serde(skip)]` like the other Duration fields (figment integration deferred).
   - `IronDeferBuilder` gains `.sweeper_interval(Duration)` setter.

6. **`TaskRepository` port extended with `recover_zombie_tasks`**:
   - New method: `async fn recover_zombie_tasks(&self) -> Result<Vec<TaskId>, TaskError>` — executes both recovery queries (retryable + exhausted) in a single call, returns IDs of all recovered/failed tasks.
   - `MockTaskRepository` automatically updated via `#[automock]`.

7. **`PostgresTaskRepository` implements `recover_zombie_tasks`**:
   - Two SQL statements matching Architecture D3.1 exactly:
     - Retryable: `UPDATE tasks SET status='pending', claimed_by=NULL, claimed_until=NULL, scheduled_at=now(), updated_at=now() WHERE status='running' AND claimed_until < now() AND attempts < max_attempts RETURNING id;`
     - Exhausted: `UPDATE tasks SET status='failed', last_error='lease expired: max attempts exhausted', updated_at=now() WHERE status='running' AND claimed_until < now() AND attempts >= max_attempts RETURNING id;`
   - Both queries leverage `idx_tasks_zombie` partial index.
   - Run the two queries sequentially (not in a transaction — each is independently correct and idempotent).

8. **`IronDefer::start()` spawns both worker pool AND sweeper**:
   - Creates a root `CancellationToken` child for the sweeper (separate from worker pool token — both cloned from the same parent `token` argument).
   - Spawns `SweeperService::run()` via `tokio::spawn`.
   - Awaits `WorkerService::run()` in the foreground.
   - After worker pool exits (cancellation), cancels the sweeper token and joins the sweeper handle.
   - If the sweeper panics, logs the error — does NOT propagate as a fatal error.

9. **Application-layer unit tests** in `crates/application/src/services/sweeper.rs` (inline `#[cfg(test)]`):
   - **`sweeper_calls_recover_on_interval`** — mock `recover_zombie_tasks` returns `Ok(vec![])`, verify it's called at least twice within 3× interval, then cancel.
   - **`sweeper_stops_on_cancellation`** — cancel token immediately, verify `run()` returns promptly.
   - **`sweeper_logs_recovery_count`** — mock returns `Ok(vec![TaskId::new(), TaskId::new()])`, verify info log emitted (use `tracing-test` or check return value).

10. **Integration test** in `crates/api/tests/worker_integration_test.rs` (extend existing or create new):
    - **`sweeper_recovers_zombie_task`** (TEA P0-INT-008-010) — submit a task, claim it (simulating a worker), let the lease expire WITHOUT completing it, start the sweeper, verify the task returns to `Pending` status and is subsequently claimed and completed by a real worker.
    - **`sweeper_fails_exhausted_zombie`** — submit a task with `max_attempts=1`, claim it (incrementing attempts to 1), let lease expire, run sweeper, verify task transitions to `Failed` with correct `last_error`.
    - Use testcontainers with the existing `test_pool()` pattern.

11. **Quality gates pass:**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace`
    - `cargo deny check bans`

## Tasks / Subtasks

- [x] **Task 1: Extend `TaskRepository` port with `recover_zombie_tasks`** (AC 6)
  - [x] Add `async fn recover_zombie_tasks(&self) -> Result<Vec<TaskId>, TaskError>` to the `TaskRepository` trait in `crates/application/src/ports/task_repository.rs`.
  - [x] Run `cargo check -p iron-defer-application` — `MockTaskRepository` auto-generates via `#[automock]`.

- [x] **Task 2: Implement `recover_zombie_tasks` in `PostgresTaskRepository`** (AC 7)
  - [x] In `crates/infrastructure/src/adapters/postgres_task_repository.rs`, implement `recover_zombie_tasks`.
  - [x] First query (retryable): `UPDATE tasks SET status='pending', claimed_by=NULL, claimed_until=NULL, scheduled_at=now(), updated_at=now() WHERE status='running' AND claimed_until < now() AND attempts < max_attempts RETURNING id;`
  - [x] Second query (exhausted): `UPDATE tasks SET status='failed', last_error='lease expired: max attempts exhausted', updated_at=now() WHERE status='running' AND claimed_until < now() AND attempts >= max_attempts RETURNING id;`
  - [x] Combine returned IDs into a single `Vec<TaskId>`.
  - [x] Add `#[instrument(skip(self), err)]` to the method.
  - [x] Run `cargo sqlx prepare --workspace` to update the `.sqlx/` offline cache.
  - [x] Run `cargo check -p iron-defer-infrastructure`.

- [x] **Task 3: Extend `WorkerConfig` with `sweeper_interval`** (AC 5)
  - [x] Add `pub sweeper_interval: Duration` to `WorkerConfig` in `crates/application/src/config.rs`.
  - [x] Add `#[serde(skip)]` attribute (consistent with other Duration fields).
  - [x] Set default to `Duration::from_secs(60)` in the `Default` impl.
  - [x] Update the `default_app_config_is_constructible` test to assert the new default.
  - [x] Run `cargo test -p iron-defer-application`.

- [x] **Task 4: Create `SweeperService`** (AC 1, 2, 3, 4)
  - [x] Create `crates/application/src/services/sweeper.rs` with `SweeperService` struct.
  - [x] Fields: `repo: Arc<dyn TaskRepository>`, `interval: Duration`, `token: CancellationToken`.
  - [x] Implement `SweeperService::new(repo, interval, token) -> Self`.
  - [x] Implement `pub async fn run(&self) -> Result<(), TaskError>` with `tokio::select!` + `interval.tick()` pattern (same structure as `WorkerService::run`).
  - [x] On each tick: call `self.repo.recover_zombie_tasks().await`, log recovered count at `info!` level.
  - [x] On error from `recover_zombie_tasks`: log at `error!` level and continue (do NOT propagate — sweeper must keep running).
  - [x] Add `pub mod sweeper;` and `pub use sweeper::SweeperService;` to `crates/application/src/services/mod.rs`.
  - [x] Re-export `SweeperService` from `crates/application/src/lib.rs`.
  - [x] Add `#[instrument(skip(self), fields(interval_secs = %self.interval.as_secs()), err)]` to `run()`.

- [x] **Task 5: Wire sweeper into `IronDefer::start()`** (AC 8)
  - [x] In `crates/api/src/lib.rs`, modify `start()`:
    - Create `PostgresTaskRepository` once, wrap in `Arc<dyn TaskRepository>`, share between worker and sweeper.
    - Clone the `token` for the sweeper (both worker and sweeper get children of the same token).
    - Construct `SweeperService::new(repo.clone(), self.worker_config.sweeper_interval, token.clone())`.
    - `tokio::spawn(async move { sweeper.run().await })` — spawn sweeper as background task.
    - Await `worker.run().await` in foreground.
    - After worker exits, the token is already cancelled — join the sweeper handle.
    - If sweeper panicked, log error but return `Ok(())` from `start()`.
  - [x] Add `.sweeper_interval(duration: Duration)` to `IronDeferBuilder` — sets `worker_config.sweeper_interval`.
  - [x] Update doc comment on `start()` to mention sweeper spawning.

- [x] **Task 6: Application-layer unit tests for `SweeperService`** (AC 9)
  - [x] `sweeper_calls_recover_on_interval` — mock returns `Ok(vec![])`, run for ~3× interval, cancel, assert ≥2 calls.
  - [x] `sweeper_stops_on_cancellation` — cancel immediately, assert `run()` returns within 1 second.
  - [x] `sweeper_continues_on_error` — mock returns `Err(TaskError::Storage { ... })`, verify sweeper does NOT exit, continues on next tick.

- [x] **Task 7: Integration tests** (AC 10)
  - [x] **`sweeper_recovers_zombie_task`** in `crates/api/tests/worker_integration_test.rs`:
    - Submit a task via `engine.enqueue(...)`.
    - Directly call `repo.claim_next(queue, worker_id, Duration::from_millis(100))` to claim with a very short lease (100ms).
    - Sleep 200ms to let the lease expire.
    - Start `SweeperService` with short interval (100ms) and let it run one cycle.
    - Query the task via raw SQL — verify `status = 'pending'`, `claimed_by IS NULL`, `claimed_until IS NULL`.
    - Start a real worker, verify the task is claimed and completed.
  - [x] **`sweeper_fails_exhausted_zombie`** in same file:
    - Submit a task with `max_attempts = 1`.
    - Claim it (this sets `attempts = 1`), let lease expire.
    - Run sweeper cycle.
    - Query via raw SQL — verify `status = 'failed'`, `last_error = 'lease expired: max attempts exhausted'`.
  - [x] Use the existing testcontainers `test_pool()` / `OnceCell` pattern.

- [x] **Task 8: Quality gates** (AC 11)
  - [x] `cargo fmt --check`
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
  - [x] `SQLX_OFFLINE=true cargo test --workspace`
  - [x] `cargo deny check bans`
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` returns empty.

## Dev Notes

### Architecture Compliance

- **D3.1 Sweeper Architecture:** Separate `tokio::spawn`'d task with its own `tokio::time::interval`. Independent of the worker pool — NOT embedded in the claim loop. Architecture lines 371–403.
- **D6.1 Shutdown Signaling:** The sweeper holds a child `CancellationToken` cloned from the root token. When root is cancelled (SIGTERM), sweeper completes its current cycle and exits. Architecture lines 449–464.
- **C2 (CRITICAL):** The sweeper's recovery queries are idempotent SQL UPDATEs — no mid-execution cancellation concern. The `tokio::select!` pattern (check token between cycles) is correct here.
- **Zombie Recovery Queries:** Architecture lines 378–401 specify the exact SQL. Use these verbatim.
- **`idx_tasks_zombie` Index:** The partial index `ON tasks (status, claimed_until) WHERE status = 'running'` already exists from migration 001. Both recovery queries filter on `status = 'running' AND claimed_until < now()`, which matches this index. No new migration needed.

### Deferred Work Items Relevant to This Story

- **`fail()` TOCTOU race** (from 1B.2 review): The sweeper's `recover_zombie_tasks` uses separate UPDATE queries that do NOT conflict with `TaskRepository::fail()` because the sweeper only touches tasks where `claimed_until < now()` (expired leases), while `fail()` is called by the worker that currently holds the lease (before expiry). No TOCTOU risk between sweeper and worker for the same task — the lease expiry is the handoff boundary.
- **`(claimed_by, claimed_until)` cross-field invariant** (from 1A.2 review): The sweeper's retryable query clears BOTH `claimed_by` and `claimed_until` together, maintaining the invariant. The exhausted query transitions to `Failed` — `claimed_by`/`claimed_until` become irrelevant for terminal states.
- **No jitter on retry backoff** (from 1B.2 review): The sweeper sets `scheduled_at = now()` for recovered tasks (immediate re-availability). Jitter is a separate concern for the `fail()` backoff formula, not the sweeper.

### OTel Metric Stub Strategy

The acceptance criteria mention `iron_defer_zombie_recoveries_total` counter. Epic 3 (Story 3.2) will wire OTel metrics. For this story:
- `recover_zombie_tasks()` returns `Vec<TaskId>` — the count is derivable from `ids.len()`.
- `SweeperService::run()` logs the count at `info!` level after each cycle.
- Epic 3 will add a `Meter` parameter to `SweeperService` and emit the counter. No OTel dependency needed now.

### Previous Story Intelligence (from Stories 1B.1, 1B.2, 1B.3)

**Code patterns established that MUST be followed:**
- `#[instrument(skip(self), fields(...), err)]` on every public async method. Payload NEVER in fields.
- No `unwrap()` / `expect()` / `panic!()` in `src/` outside `#[cfg(test)]`. Map all errors to `TaskError`.
- Error source chains preserved — never discard context.
- Integration tests use shared `OnceCell<Option<TestDb>>` testcontainers pattern from `crates/api/tests/common/mod.rs`.
- Load-bearing tests verify via raw SQL, not just API return values.
- `WorkerService::run()` pattern (in `crates/application/src/services/worker.rs`) is the template for `SweeperService::run()` — same `tokio::select!` + `interval.tick()` structure.

**Key types and locations (verified current):**
- `TaskRecord` — `crates/domain/src/model/task.rs` — 14 fields, `#[non_exhaustive]`
- `TaskId` — `crates/domain/src/model/task.rs` — UUID wrapper, `from_uuid()` and `as_uuid()` accessors
- `WorkerId` — `crates/domain/src/model/worker.rs` — UUID wrapper
- `QueueName` — `crates/domain/src/model/queue.rs` — validated string
- `TaskStatus` — `crates/domain/src/model/task.rs` — `Pending, Running, Completed, Failed, Cancelled`
- `TaskError` — `crates/domain/src/error.rs` — variants: `AlreadyClaimed`, `InvalidPayload`, `ExecutionFailed`, `Storage`
- `TaskRepository` trait — `crates/application/src/ports/task_repository.rs` — `#[automock]`, methods: `save`, `find_by_id`, `list_by_queue`, `claim_next`, `complete`, `fail`
- `WorkerService` — `crates/application/src/services/worker.rs` — reference implementation for `SweeperService`
- `WorkerConfig` — `crates/application/src/config.rs:34-53` — extend with `sweeper_interval`
- `IronDefer` — `crates/api/src/lib.rs` — `start()` method at line ~294
- `IronDeferBuilder` — `crates/api/src/lib.rs` — builder pattern, add `.sweeper_interval()` setter
- `PostgresTaskRepository` — `crates/infrastructure/src/adapters/postgres_task_repository.rs`

**Dependencies already available — no new crate additions expected:**
- `tokio` (with `time`, `sync`, `macros` features)
- `tokio-util` (with `CancellationToken`)
- `tracing` (`info!`, `error!`, `warn!`, `instrument`)
- `sqlx` (for Postgres queries)
- `mockall` (dev-dependency for `#[automock]`)
- `chrono`, `uuid`, `serde_json`

### Test Strategy

**Unit tests** (application layer, mock repo):
- Verify sweep loop timing and cancellation behavior.
- Verify error resilience (sweeper continues on repo errors).
- No database needed.

**Integration tests** (api layer, testcontainers Postgres):
- Verify end-to-end zombie recovery with real SQL.
- Verify retryable vs exhausted task handling.
- Verify recovered tasks are re-claimable by workers.
- Use short lease durations (100ms) and sweep intervals (100ms) for fast tests.

### Project Structure Notes

- **New files:**
  - `crates/application/src/services/sweeper.rs` — `SweeperService` implementation + unit tests
- **Modified files:**
  - `crates/application/src/services/mod.rs` — add `pub mod sweeper;` + re-export
  - `crates/application/src/lib.rs` — re-export `SweeperService`
  - `crates/application/src/config.rs` — add `sweeper_interval` to `WorkerConfig`
  - `crates/application/src/ports/task_repository.rs` — add `recover_zombie_tasks` method
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — implement `recover_zombie_tasks`
  - `crates/api/src/lib.rs` — modify `start()` to spawn sweeper, add `.sweeper_interval()` builder method
  - `crates/api/tests/worker_integration_test.rs` — add sweeper integration tests
  - `.sqlx/` — updated offline cache after `cargo sqlx prepare`

### Out of Scope

- **OTel metric emission** — Epic 3, Story 3.2 wires the `iron_defer_zombie_recoveries_total` counter.
- **Graceful shutdown with drain timeout** — Story 2.2 adds `shutdown.rs` with timeout enforcement and lease release on SIGTERM.
- **Exponential backoff on sweeper recovery** — Architecture D3.1 specifies `scheduled_at = now()` (immediate re-availability). Backoff is handled by `TaskRepository::fail()` in the worker path, not the sweeper.
- **`main.rs` wiring** — Epic 4 standalone binary. `start()` is the library entry point.

### References

- [Source: `docs/artifacts/planning/architecture.md` §D3.1 lines 371–403] — Sweeper architecture, zombie recovery SQL
- [Source: `docs/artifacts/planning/architecture.md` §D6.1 lines 449–464] — CancellationToken tree, shutdown signaling
- [Source: `docs/artifacts/planning/architecture.md` §D1.2 lines 307–318] — Retry / backoff formula (for context)
- [Source: `docs/artifacts/planning/architecture.md` lines 294–297] — `idx_tasks_zombie` partial index definition
- [Source: `docs/artifacts/planning/architecture.md` line 967] — Sweeper maps to `application/services/worker.rs (SweeperService)`
- [Source: `docs/artifacts/planning/epics.md` lines 491–526] — Story 2.1 acceptance criteria
- [Source: `docs/artifacts/planning/prd.md` §FR13, FR15, FR43] — Functional requirements
- [Source: `docs/artifacts/planning/prd.md` §NFR-R2] — Sweeper recovers all zombies within 2× configured interval
- [Source: `docs/artifacts/implementation/1b-2-worker-pool-and-execution-loop.md`] — WorkerService pattern (template for SweeperService)
- [Source: `docs/artifacts/implementation/deferred-work.md`] — Deferred items relevant to sweeper

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None — clean implementation with no blockers or failures.

### Completion Notes List

- **Task 1:** Added `recover_zombie_tasks` method to `TaskRepository` trait. `MockTaskRepository` auto-updated via `#[automock]`.
- **Task 2:** Implemented `recover_zombie_tasks` in `PostgresTaskRepository` with two sequential SQL UPDATE queries matching Architecture D3.1 exactly. Retryable tasks reset to `Pending`, exhausted tasks transition to `Failed`. Updated `.sqlx/` offline cache via `cargo sqlx prepare --workspace`.
- **Task 3:** Extended `WorkerConfig` with `sweeper_interval: Duration` (default 60s, `#[serde(skip)]`). Updated default test assertion. Fixed `fast_config()` test helper in `worker.rs` to include new field.
- **Task 4:** Created `SweeperService` in `crates/application/src/services/sweeper.rs` with `tokio::select!` + `interval.tick()` pattern (modeled after `WorkerService::run()`). Errors logged at `error!` level, sweeper continues on next tick. Recovery count logged at `info!` level when > 0.
- **Task 5:** Modified `IronDefer::start()` to spawn `SweeperService` as a background `tokio::spawn`'d task alongside the worker pool. Both share the same `CancellationToken`. Sweeper handle joined after worker exits. Sweeper panics logged but not propagated. Added `.sweeper_interval()` builder method.
- **Task 6:** 3 unit tests: `sweeper_calls_recover_on_interval` (verifies ≥2 calls within 3× interval), `sweeper_stops_on_cancellation` (verifies prompt exit), `sweeper_continues_on_error` (verifies resilience to repo errors).
- **Task 7:** 2 integration tests with testcontainers: `sweeper_recovers_zombie_task` (LOAD-BEARING — submit, claim with 100ms lease, expire, sweep, verify Pending via raw SQL, then re-claim and complete), `sweeper_fails_exhausted_zombie` (max_attempts=1, claim, expire, sweep, verify Failed with correct `last_error` via raw SQL).
- **Task 8:** All quality gates pass: `cargo fmt --check`, `cargo clippy --pedantic`, 101 tests (19 application unit including 3 new sweeper + 17 infra integration + 5 worker integration including 2 new sweeper + remaining), `cargo deny check bans`, no openssl/native-tls in production graph.

### File List

- `crates/application/src/services/sweeper.rs` — new (SweeperService + 3 unit tests)
- `crates/application/src/services/mod.rs` — modified (add `pub mod sweeper` + re-export)
- `crates/application/src/lib.rs` — modified (re-export `SweeperService`)
- `crates/application/src/config.rs` — modified (add `sweeper_interval` to `WorkerConfig`)
- `crates/application/src/ports/task_repository.rs` — modified (add `recover_zombie_tasks` method)
- `crates/application/src/services/worker.rs` — modified (add `sweeper_interval` to test `fast_config()`)
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — modified (implement `recover_zombie_tasks`)
- `crates/api/src/lib.rs` — modified (spawn sweeper in `start()`, add `.sweeper_interval()` builder)
- `crates/api/tests/worker_integration_test.rs` — modified (add 2 sweeper integration tests)
- `.sqlx/` — updated (2 new query cache entries for sweeper SQL)

### Review Findings

- [x] [Review][Patch] `token.clone()` should be `token.child_token()` — D6.1 requires separate child tokens for sweeper and worker so Story 2.2 can cancel them independently during drain timeout. Fixed: `start()` now creates `token.child_token()` for both sweeper and worker. [crates/api/src/lib.rs]
- [x] [Review][Patch] Missing `sweeper_logs_recovery_count` unit test (AC 9) — AC 9 requires a test that verifies info log when mock returns recovered IDs. Fixed: added `sweeper_logs_recovery_count` test alongside existing `sweeper_continues_on_error`. [crates/application/src/services/sweeper.rs]
- [x] [Review][Patch] Misleading test comment `"zombie recovery + real worker"` — sweeper recovery does NOT increment attempts. Fixed: comment now clarifies attempts = 1 (direct claim) + 1 (worker re-claim), sweeper preserves retry budget. [crates/api/tests/worker_integration_test.rs]
- [x] [Review][Defer] Non-atomic two-query `recover_zombie_tasks` — partial DB failure between the two UPDATEs leaves exhausted zombies in Running for one extra sweep cycle. Story spec explicitly says "Run the two queries sequentially (not in a transaction — each is independently correct and idempotent)". Eventually consistent. [crates/infrastructure/src/adapters/postgres_task_repository.rs] — deferred, by design
- [x] [Review][Defer] `Duration::ZERO` for `sweeper_interval` panics in spawned task — same pre-existing pattern as `poll_interval` (already tracked in deferred-work.md). Add validation when figment config integration lands. [crates/application/src/config.rs] — deferred, pre-existing

### Change Log

- 2026-04-13: Implemented Story 2.1 — Sweeper zombie task recovery. `SweeperService` runs as independent background task, recovers expired-lease tasks (retryable → Pending, exhausted → Failed). Wired into `IronDefer::start()` alongside worker pool. 5 new tests (3 unit + 2 integration). Total: 101 tests passing, all quality gates green.
