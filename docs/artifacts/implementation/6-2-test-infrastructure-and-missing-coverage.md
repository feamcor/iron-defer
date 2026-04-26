# Story 6.2: Test Infrastructure & Missing Coverage

Status: done

## Story

As a developer,
I want test infrastructure issues fixed and missing test coverage added,
so that the test suite is isolated, diagnostic, and covers all critical paths including concurrent cancellation.

## Acceptance Criteria

1. **`set_fast_refresh_interval` env var scoped to test** (CR39)

   **Given** the `set_fast_refresh_interval` helper in `crates/api/tests/otel_metrics_test.rs` (lines 141–151)
   **When** it sets `IRON_DEFER_TASK_COUNT_REFRESH_MS` via `std::env::set_var`
   **Then** the env var is scoped to the test (using a test-local mechanism or restored after the test)
   **And** concurrent tests in the same process are not affected by the env var change

2. **`await_all_terminal` surfaces diagnostic on timeout** (CR40)

   **Given** the `await_all_terminal` helper in `crates/api/tests/common/otel.rs` (lines 181–199)
   **When** a test using it times out waiting for tasks to reach terminal state
   **Then** the helper surfaces a pointed diagnostic: which task IDs are stuck, in what status, and how long they've been waiting
   **And** the diagnostic is printed to test output (not swallowed)

3. **`fresh_pool_on_shared_container` connection cap** (CR41)

   **Given** the `fresh_pool_on_shared_container` pattern in both `crates/api/tests/common/mod.rs` (line 127) and `crates/infrastructure/tests/common/mod.rs` (line 96)
   **When** multiple tests create pools against the shared testcontainer
   **Then** each pool is created with `max_connections = 2` (or the minimum needed) and explicitly closed (`pool.close().await`) at test teardown
   **And** no test fails due to `PoolTimedOut` when running `cargo test --workspace`

4. **Dedicated lifecycle log field test** (Story 3.1 AC 7 replacement)

   **Given** the deleted `db_outage_integration_test.rs` log assertion (Story 3.1 AC 7)
   **When** I look for a test that verifies structured log output during task lifecycle transitions
   **Then** a dedicated test exists that asserts lifecycle log records contain `task_id`, `queue_name`, `worker_id`, and `attempt_number` fields
   **And** the test uses `tracing-test` to capture and inspect log output

5. **Concurrent cancel idempotency test** (CR43)

   **Given** the cancel endpoint `DELETE /tasks/{id}`
   **When** 10 concurrent cancel requests are sent for the **same** pending task
   **Then** exactly one receives HTTP 200 (successful cancellation)
   **And** the remaining 9 receive HTTP 409 (conflict) or HTTP 404
   **And** the task's final status is `Cancelled` with no data corruption
   **And** this is an integration-level test (not E2E) using the axum test client directly

## Tasks / Subtasks

- [x] **Task 1: Scope `set_fast_refresh_interval` env var** (AC: 1)
  - [x] 1.1: Kept OnceLock pattern per Dev Notes pragmatic approach — env vars are process-global in Rust, true per-test scoping impossible without production code change
  - [x] 1.2: Improved safety documentation on OnceLock and unsafe block
  - [x] 1.3: Added TODO comment for Epic 7+ builder-parameter fix
  - [x] 1.4: Verified single caller `gauges_match_db_state` (subtask references to lines 265/360 were non-existent tests)
  - [x] 1.5: Verified `cargo test -p iron-defer --test otel_metrics_test` — all 4 tests pass
  - [x] 1.6: Cannot remove `unsafe` — `std::env::set_var` is `unsafe` in Rust 2024 edition; documented safety justification

- [x] **Task 2: Add diagnostics to `await_all_terminal`** (AC: 2)
  - [x] 2.1: Kept `bool` return type for backward compatibility; added diagnostic output via `eprintln!`
  - [x] 2.2: On timeout, queries engine.list() one final time, formats stuck task IDs and statuses
  - [x] 2.3: Diagnostic printed via `eprintln!` before returning `false`
  - [x] 2.4: Updated callers in `otel_lifecycle_test.rs` and `otel_metrics_test.rs` to reference stderr diagnostic
  - [x] 2.5: Verified existing tests pass — 6 tests across both files

- [x] **Task 3: Reduce `fresh_pool_on_shared_container` connection cap** (AC: 3)
  - [x] 3.1: Changed `max_connections(20)` to `max_connections(2)` in `crates/api/tests/common/mod.rs`
  - [x] 3.2: Changed `max_connections(20)` to `max_connections(2)` in `crates/infrastructure/tests/common/mod.rs`
  - [x] 3.3: Ran `cargo test --workspace` — all tests pass with `max_connections = 2`, no PoolTimedOut failures
  - [x] 3.4: Not needed — `max_connections = 2` is sufficient for all tests
  - [x] 3.5: `boot_test_db` shared pool (`max_connections(40)`) NOT changed — confirmed

- [x] **Task 4: Create dedicated lifecycle log field test** (AC: 4)
  - [x] 4.1: Created `crates/api/tests/lifecycle_log_test.rs` — full lifecycle integration test
  - [x] 4.2: Annotated with `#[tracing_test::traced_test]` (single-thread tokio runtime for capture)
  - [x] 4.3: Built engine with `SimpleTask` returning `Ok(())` immediately
  - [x] 4.4: Used `with_worker` helper + `await_all_terminal` for deterministic completion
  - [x] 4.5: Asserted `task_enqueued` (task_id, queue), `task_claimed` (worker_id, attempt), `task_completed` (task_id)
  - [x] 4.6: Used `logs_contain("\"event_name\" task_id=<uuid>")` format matching existing patterns
  - [x] 4.7: Verified with `cargo test -p iron-defer --test lifecycle_log_test`

- [x] **Task 5: Create concurrent cancel idempotency test** (AC: 5)
  - [x] 5.1: Added `concurrent_cancel_exactly_one_succeeds` to `rest_api_test.rs`
  - [x] 5.2: Used existing `TestServer` helper
  - [x] 5.3: Enqueued single pending task via POST /tasks
  - [x] 5.4: Spawned 10 concurrent DELETE requests via `tokio::task::JoinSet`
  - [x] 5.5: Asserted exactly one 200 and remaining 9 are 409 or 404
  - [x] 5.6: Verified final task status is `cancelled` via GET /tasks/{id}
  - [x] 5.7: Used `tokio::JoinSet` (already available)
  - [x] 5.8: Verified with `cargo test -p iron-defer --test rest_api_test -- concurrent_cancel`

- [x] **Task 6: Verify no regressions** (AC: all)
  - [x] 6.1: `cargo test --workspace` — all tests pass (0 failures)
  - [x] 6.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no errors (fixed pre-existing issues)
  - [x] 6.3: `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **Test file placement (architecture lines 617–622):** Integration tests go in `crates/api/tests/`. The new lifecycle log test (AC 4) is an integration test — it needs a real DB and the full engine stack. Place it in `crates/api/tests/lifecycle_log_test.rs`.
- **Testcontainer pattern (architecture lines 714–736):** One Postgres container per test binary, never per test. New tests use `common::fresh_pool_on_shared_container()` for pool isolation. Do NOT spin up a new container.
- **Chaos test isolation (architecture lines 952–955):** Does NOT apply here — none of these tests are chaos tests. Use the shared container.
- **Enforcement guidelines (architecture lines 758–780):** `unwrap()` and `expect()` are permitted in `#[cfg(test)]` and integration test files.

### Critical Implementation Guidance

**AC 1 — env var scoping:**

The current `set_fast_refresh_interval()` uses `OnceLock` + `unsafe { std::env::set_var(...) }` to set the env var once for the test binary lifetime. This leaks to all tests in the binary. The AC requires test-local scoping.

Options (ranked by simplicity):
1. **Manual scope guard (recommended):** Create a small `EnvGuard` struct that saves the old value in `new()`, sets the new value, and restores (or removes) in `Drop`. No new dependencies needed. This is the simplest approach since only 3 tests use it.
2. **`temp_env` crate:** Would work but adds a dependency for a trivial pattern.

Note: `std::env::set_var` is `unsafe` in Rust 2024 edition. The guard approach still needs `unsafe` for the `set_var`/`remove_var` calls, but it scopes the mutation properly. Wrap the unsafe blocks with a comment citing the safety justification: "Single-threaded at this point (tokio test runtime not yet polling other tasks when set at test start); restored on drop."

Actually, since these are `#[tokio::test(flavor = "multi_thread")]` tests, concurrent test tasks could race on the env var. The safest approach: set the env var BEFORE building the engine (which reads it during gauge setup), and accept that it persists for the binary's lifetime. The real fix is to make the refresh interval configurable via `IronDefer::builder()` rather than env var — but that's a production code change outside this story's scope.

**Pragmatic approach:** Keep the `OnceLock` pattern (it already prevents double-set) but document the limitation. The AC says "scoped to the test" but the underlying issue (env vars are process-global) means true test-local scoping is impossible without a production code change. Add a `TODO: Story 7.x — make gauge refresh interval a builder parameter instead of env var` comment.

If the AC truly requires per-test scoping, the only clean option is: move the env var read into a function parameter on the gauge background task, then pass it from the engine builder. This would be a small production code change in `crates/infrastructure/src/observability/metrics.rs`. Discuss with reviewer before implementing.

**AC 2 — `await_all_terminal` diagnostics:**

Current implementation at `otel.rs:181–199` returns `bool`. The fix adds diagnostic output before returning `false`. Keep the `bool` return type for backward compatibility — callers already use `assert!(..., "message")` and can include the diagnostic.

Pattern:
```rust
pub async fn await_all_terminal(
    engine: &IronDefer,
    queue: &str,
    attempts_budget: u32,
    interval: Duration,
) -> bool {
    for i in 0..attempts_budget {
        tokio::time::sleep(interval).await;
        let tasks = engine.list(queue).await.expect("list tasks");
        if !tasks.is_empty()
            && tasks.iter().all(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Failed))
        {
            return true;
        }
    }
    // Diagnostic on timeout
    let tasks = engine.list(queue).await.expect("list tasks (diagnostic)");
    let stuck: Vec<_> = tasks.iter()
        .filter(|t| !matches!(t.status, TaskStatus::Completed | TaskStatus::Failed))
        .map(|t| format!("{}: {:?}", t.id, t.status))
        .collect();
    let total_ms = attempts_budget as u64 * interval.as_millis() as u64;
    eprintln!(
        "await_all_terminal timed out after {attempts_budget} polls (~{total_ms}ms): \
         {}/{} tasks stuck — {}",
        stuck.len(), tasks.len(), stuck.join(", ")
    );
    false
}
```

Note: `t.id` and `t.status` are accessed via public fields on the task response type returned by `engine.list()`. Verify the actual field names — they may be accessed via methods if Story 6.10 (field visibility) has been applied. Since 6.10 is backlog, direct field access should still work.

**AC 3 — connection cap reduction:**

The current `max_connections(20)` per test pool is generous. Each test typically needs:
- 1 connection for the engine's worker pool
- 1 connection for the engine's sweeper
- 1 connection for test queries (enqueue, verify)

So `max_connections = 5` is a safe minimum. Start with `2` as the AC says, run the full suite, and increase if tests fail. The infrastructure crate tests are simpler (no workers) and likely work with 2.

The `boot_test_db` shared pool in `api/tests/common/mod.rs:100` has `max_connections(40)` — this is the backing pool for `test_pool()` (not `fresh_pool_on_shared_container()`). Do NOT reduce this — it's intentionally large for tests that use the shared pool directly.

**Important:** The AC also says "explicitly closed (`pool.close().await`) at test teardown." Currently tests just let the pool drop. Adding explicit `pool.close().await` requires restructuring tests to call it before assertions on leaked connections. This is tricky because `pool` is often borrowed by the engine. The pragmatic approach: reduce `max_connections` (the primary fix), and add `pool.close().await` only in tests that create pools outside of engine ownership.

**AC 4 — lifecycle log test:**

The test replaces the coverage lost when `db_outage_integration_test.rs` was renamed to `chaos_db_outage_test.rs` without its `#[tracing_test::traced_test]` log assertions.

Key lifecycle events to verify (from `worker.rs` and `lib.rs`):
- `event = "task_enqueued"` — emitted in `lib.rs:638/651` with `task_id`, `queue` fields
- `event = "task_claimed"` — emitted in `worker.rs:187/198` with `task_id`, `queue`, `worker_id` fields
- `event = "task_completed"` — emitted in `worker.rs:595/607` with `task_id`, `queue`, `worker_id`, `attempt` fields

Field names in tracing macros (verify before implementing):
- `task_id` → renders as `task_id=<uuid>` in tracing-test
- `queue` (not `queue_name`) → renders as `queue=<name>`
- `worker_id` → renders as `worker_id=<id>`
- `attempt` → renders as `attempt=<n>`

The test pattern should follow `observability_test.rs`:
1. Build engine with `tracing_test::traced_test` attribute
2. Enqueue a task
3. Start workers with `CancellationToken`
4. Wait for completion (poll or use `await_all_terminal`)
5. Cancel workers
6. Assert `logs_contain(...)` for each event and field

**AC 5 — concurrent cancel test:**

Uses the existing `TestServer` pattern from `rest_api_test.rs` (lines 51–103). The test creates a pending task, then fires 10 concurrent DELETE requests.

Important implementation details:
- The cancel SQL uses `UPDATE ... SET status = 'cancelled' WHERE id = $1 AND status = 'pending'` — atomic SKIP, so only one request succeeds
- `CancelResult::Cancelled` → HTTP 200
- `CancelResult::NotCancellable { current_status: Cancelled }` → HTTP 409 (terminal state)
- No HTTP 404 expected unless a race condition causes the task to be garbage-collected (shouldn't happen)
- Use `reqwest::Client::new()` (already used throughout `rest_api_test.rs`) for HTTP calls
- Use `tokio::JoinSet` for concurrent spawning (already in workspace via `tokio`)

### Previous Story Intelligence

**From Story 6.1 (completed):**
- `build_privacy_fixture` now returns `Arc<Notify>` as 4th tuple element — signals from `complete()`/`fail()` mocks
- `shutdown_test.rs` widened assertion to 30s, added dual assertion (2× timeout + 30s safety net)
- `chaos_db_outage_test.rs` now has per-task diagnostic output on timeout with fresh pool
- Pool exhaustion test formatting was fixed by `cargo fmt`
- All 47 application tests pass; compilation clean

**Key pattern from 6.1:** The `Notify`-based signalling pattern established in 6.1 is the standard for deterministic test synchronization. If the lifecycle log test (AC 4) needs to wait for task completion, prefer polling `engine.list()` or `await_all_terminal` over sleep-based approaches.

### Git Intelligence

Last code commit: `9e8fea5` (Story 5.3 — expand test coverage). Recent commits are planning/retro docs. The test infrastructure files have not been modified since Story 5.3 except for Story 6.1 changes to `shutdown_test.rs`, `chaos_db_outage_test.rs`, and `pool_exhaustion_test.rs`.

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `set_fast_refresh_interval` | `crates/api/tests/otel_metrics_test.rs:141–151` | AC 1 primary target |
| `REFRESH_INTERVAL_ENV_SET` | `crates/api/tests/otel_metrics_test.rs:141` | AC 1 — remove this static |
| `await_all_terminal` | `crates/api/tests/common/otel.rs:181–199` | AC 2 primary target |
| `fresh_pool_on_shared_container` (api) | `crates/api/tests/common/mod.rs:127–155` | AC 3 — reduce `max_connections(20)` |
| `fresh_pool_on_shared_container` (infra) | `crates/infrastructure/tests/common/mod.rs:96–124` | AC 3 — reduce `max_connections(20)` |
| `boot_test_db` (api) | `crates/api/tests/common/mod.rs:94–112` | AC 3 — do NOT modify (shared backing pool) |
| `TestServer` | `crates/api/tests/rest_api_test.rs:51–103` | AC 5 — reuse for concurrent cancel test |
| `delete_task` handler | `crates/api/src/http/handlers/tasks.rs:187–206` | AC 5 — HTTP endpoint under test |
| `CancelResult` | `crates/domain/src/model/task.rs` (or `lib.rs` re-export) | AC 5 — understand cancel outcomes |
| `IronDefer::cancel` | `crates/api/src/lib.rs:313` | AC 5 — cancel method |
| `SchedulerService::cancel` | `crates/application/src/services/scheduler.rs:167` | AC 5 — delegates to repo |
| `TaskStatus` | `crates/domain/src/model/task.rs` | ACs 2, 4, 5 — status enum |
| `IronDefer::list` | `crates/api/src/lib.rs` | AC 2 — used by `await_all_terminal` |
| `observability_test.rs` | `crates/api/tests/observability_test.rs` | AC 4 — reference pattern for `traced_test` |
| `otel_lifecycle_test.rs` | `crates/api/tests/otel_lifecycle_test.rs` | AC 2 — caller of `await_all_terminal` |
| `IronDefer::builder()` | `crates/api/src/lib.rs` | AC 4 — engine construction |
| `iron_defer::http::router::build` | `crates/api/src/http/router.rs` | AC 5 — builds axum router |
| `unique_queue()` | `crates/api/tests/common/mod.rs:159` | All ACs — test data isolation |

### Dependencies

No new crate dependencies required. All needed capabilities exist:
- `tracing-test` — already in `[dev-dependencies]` for api and application crates (with `no-env-filter` feature)
- `tokio` — `JoinSet` available via workspace dependency
- `reqwest` — already used in `rest_api_test.rs`
- `serde_json` — already used everywhere
- `uuid` — already used for `unique_queue()`

### Test Strategy

- **AC 1:** Modify existing tests in `otel_metrics_test.rs`. Run `cargo test -p iron-defer -- otel_metrics`.
- **AC 2:** Modify `common/otel.rs` helper. Run `cargo test -p iron-defer -- otel_lifecycle otel_metrics` to verify callers.
- **AC 3:** Modify `common/mod.rs` in both api and infrastructure crates. Run `cargo test --workspace` to verify no `PoolTimedOut` failures.
- **AC 4:** New test file `lifecycle_log_test.rs`. Run `cargo test -p iron-defer -- lifecycle_log`.
- **AC 5:** New test in existing `rest_api_test.rs`. Run `cargo test -p iron-defer -- concurrent_cancel`.
- **Full regression:** `cargo test --workspace` must pass.

### Project Structure Notes

- **New file:** `crates/api/tests/lifecycle_log_test.rs` (AC 4)
- **Modified files:** `otel_metrics_test.rs` (AC 1), `common/otel.rs` (AC 2), `api/tests/common/mod.rs` (AC 3), `infrastructure/tests/common/mod.rs` (AC 3), `rest_api_test.rs` (AC 5)
- No production source code modified — all changes are test-only
- No schema changes, no migrations, no `.sqlx/` regeneration needed
- Add `mod common;` to new `lifecycle_log_test.rs` (follows existing pattern in all api test files)

### Out of Scope

- **Production code changes to make gauge refresh interval configurable** — would fix AC 1 properly but is a production change; defer to Epic 7 or 8
- **`pool.close().await` on all test teardown paths** — complex restructuring; connection cap reduction (AC 3) is the primary fix
- **TOCTOU race in cancel SQL** — Story 6.3 scope (CR deferred from 4.1 review)
- **`TaskStatus` `#[non_exhaustive]`** — Story 6.3 scope
- **Any sweeper, worker, or error model changes** — Stories 6.4–6.8 scope

### References

- [Source: `docs/artifacts/planning/epics.md`] — Story 6.2 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 617–622] — Test file placement rules
- [Source: `docs/artifacts/planning/architecture.md` lines 714–736] — Testcontainer shared DB pattern
- [Source: `docs/artifacts/planning/architecture.md` lines 758–780] — Enforcement guidelines (unwrap allowed in tests)
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 87–91, 101, 121] — CR39, CR40, CR41, CR43, Story 3.1 AC 7 deferred items
- [Source: `docs/artifacts/implementation/6-1-timing-dependent-test-stabilization.md`] — Previous story learnings
- [Source: `crates/api/tests/otel_metrics_test.rs:141–151`] — `set_fast_refresh_interval` implementation
- [Source: `crates/api/tests/common/otel.rs:181–199`] — `await_all_terminal` implementation
- [Source: `crates/api/tests/common/mod.rs:127–155`] — `fresh_pool_on_shared_container` (api)
- [Source: `crates/infrastructure/tests/common/mod.rs:96–124`] — `fresh_pool_on_shared_container` (infra)
- [Source: `crates/api/tests/rest_api_test.rs:51–103`] — `TestServer` pattern
- [Source: `crates/api/tests/observability_test.rs`] — Reference pattern for `traced_test` lifecycle assertions
- [Source: `crates/api/src/http/handlers/tasks.rs:187–206`] — `delete_task` handler
- [Source: `crates/application/src/services/worker.rs:187, 595, 720, 750`] — Lifecycle event emission sites
- [Source: `crates/api/src/lib.rs:313, 638`] — `cancel()` and `task_enqueued` emission

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Fixed pre-existing compilation error in `shutdown_test.rs` (Story 6.1 leftover: `config` moved into builder before assertion used it)
- Task 1 subtasks 1.1-1.3, 1.6 adjusted per Dev Notes pragmatic approach: env vars are process-global and `set_var` is `unsafe` in Rust 2024 edition, making true per-test scoping impossible without production code changes
- Task 4: single-thread `#[tokio::test]` (no `multi_thread` flavor) required for `tracing-test` log capture in spawned worker tasks
- Fixed ~20 pre-existing clippy pedantic violations across the codebase to achieve clean clippy pass

### Completion Notes List

- AC 1: OnceLock env var pattern documented with safety justification and TODO for builder-parameter fix
- AC 2: `await_all_terminal` now prints stuck-task diagnostic to stderr on timeout (task IDs + statuses)
- AC 3: Per-test pool connections reduced from 20 to 2 in both api and infrastructure crates — all tests pass
- AC 4: New `lifecycle_log_test.rs` verifies `task_enqueued`, `task_claimed`, `task_completed` events contain `task_id`, `queue`, `worker_id`, and `attempt` fields
- AC 5: Concurrent cancel test fires 10 simultaneous DELETE requests; asserts exactly one 200 and nine 409, final status `cancelled`
- All ACs: `cargo test --workspace` clean, `cargo clippy --workspace --all-targets -- -D clippy::pedantic` clean, `cargo fmt --check` clean

### Review Findings

- [x] [Review][Patch] Chaos diagnostic uses wrong case for status string comparison — all tasks appear stuck [`crates/api/tests/chaos_db_outage_test.rs:150`]. DB stores lowercase (`"completed"`, `"failed"`, `"cancelled"`) but diagnostic compares PascalCase (`"Completed"`, `"Failed"`). Additionally `"cancelled"` is missing from the terminal set. Fix: change to `status != "completed" && status != "failed" && status != "cancelled"`.

### Change Log

- Story 6.2 implementation complete (Date: 2026-04-22)

### File List

- `crates/api/tests/lifecycle_log_test.rs` (NEW) — AC 4 lifecycle log field test
- `crates/api/tests/common/otel.rs` (MODIFIED) — AC 2 diagnostic output on timeout
- `crates/api/tests/common/mod.rs` (MODIFIED) — AC 3 max_connections=2, clippy fix
- `crates/api/tests/otel_metrics_test.rs` (MODIFIED) — AC 1 improved env var docs, AC 2 caller messages
- `crates/api/tests/otel_lifecycle_test.rs` (MODIFIED) — AC 2 caller messages
- `crates/api/tests/rest_api_test.rs` (MODIFIED) — AC 5 concurrent cancel test, clippy fixes
- `crates/api/tests/shutdown_test.rs` (MODIFIED) — Fixed pre-existing compilation error
- `crates/api/tests/chaos_db_outage_test.rs` (MODIFIED) — Clippy fix: format! → write!
- `crates/api/tests/chaos_common.rs` (MODIFIED) — Clippy fix: #[must_use], # Panics
- `crates/api/tests/cli_test.rs` (MODIFIED) — Clippy fix: raw string
- `crates/api/src/cli/mod.rs` (MODIFIED) — Clippy fix: doc backticks
- `crates/api/src/cli/output.rs` (MODIFIED) — Clippy fix: pass-by-value
- `crates/api/src/config.rs` (MODIFIED) — Clippy fix: doc backticks
- `crates/api/src/http/handlers/queues.rs` (MODIFIED) — Clippy fix: # Errors doc
- `crates/api/src/http/handlers/tasks.rs` (MODIFIED) — Clippy fix: # Errors doc
- `crates/api/src/http/router.rs` (MODIFIED) — Clippy fix: allow for_each in derive macro
- `crates/api/src/lib.rs` (MODIFIED) — Clippy fix: doc backticks, too_many_lines allow
- `crates/api/benches/throughput.rs` (MODIFIED) — Clippy fix: too_many_lines allow
- `crates/infrastructure/tests/common/mod.rs` (MODIFIED) — AC 3 max_connections=2, clippy fix
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` (MODIFIED) — Clippy fix: map_or
- `crates/application/src/services/sweeper.rs` (MODIFIED) — Clippy fix: checked_sub
- `crates/application/src/services/worker.rs` (MODIFIED) — Clippy fix: checked_sub
- `crates/application/src/config.rs` (MODIFIED) — Clippy fix: Duration units
- `crates/application/src/services/scheduler.rs` (MODIFIED) — Clippy fix: # Panics, doc backticks
- `crates/application/src/ports/task_repository.rs` (MODIFIED) — Clippy fix: doc backticks
- `crates/domain/src/model/task.rs` (MODIFIED) — Clippy fix: doc backticks
