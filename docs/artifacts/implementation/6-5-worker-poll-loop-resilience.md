# Story 6.5: Worker Poll Loop Resilience

Status: done

## Story

As a platform engineer,
I want the worker poll loop to handle claim errors gracefully and shut down without delays,
so that transient database issues don't cause thundering herd retries and planned shutdowns complete promptly.

## Acceptance Criteria

1. **Jittered backoff on consecutive claim errors (CR16)**

   **Given** the worker poll loop in `crates/application/src/services/worker.rs`
   **When** `claim_next` returns an error (e.g., `PoolTimedOut`, transient connection failure)
   **Then** the worker applies per-worker jittered backoff before the next claim attempt
   **And** the jitter formula avoids thundering herd: `base_delay + random(0..base_delay)` where `base_delay` starts at the poll interval and doubles on consecutive errors (capped at a configurable maximum)
   **And** the backoff resets to zero on a successful claim

2. **Shutdown races cancellation against stuck claim (CR19)**

   **Given** the worker poll loop during shutdown
   **When** the `CancellationToken` is cancelled while `claim_next` is blocked waiting for a pool connection (stuck on `acquire_timeout`)
   **Then** the claim attempt is raced against the cancellation token using `tokio::select!` or `tokio::time::timeout`
   **And** shutdown does not wait the full `acquire_timeout` (5s default) per worker before proceeding
   **And** the worker exits the poll loop within 1 second of token cancellation (excluding in-flight task completion)

3. **Combined verification**

   **Given** the combined changes
   **When** a shutdown signal is sent while the pool is saturated
   **Then** all workers exit promptly without the cumulative 5s-per-worker delay
   **And** no tasks are lost — pending tasks remain pending, in-flight tasks complete or leases are released

## Tasks / Subtasks

- [x] **Task 1: Add jittered backoff state to the poll loop** (AC: 1)
  - [x] 1.1: Add a `consecutive_errors: u32` counter and `backoff_until: Option<Instant>` variable before the main loop
  - [x] 1.2: In the error branch, increment `consecutive_errors` and compute the next backoff: `base_delay = min(poll_interval * 2^consecutive_errors, max_claim_backoff)`, then add jitter: `actual_delay = base_delay + rand(0..=base_delay)`
  - [x] 1.3: When backoff is active, sleep for the computed delay before the next `tick.tick()` — uses `tokio::select!` to race the backoff sleep against the cancellation token so shutdown is not delayed by backoff
  - [x] 1.4: In the `Ok(Some(task))` branch, reset `consecutive_errors = 0`
  - [x] 1.5: In the `Ok(None)` branch (no task available), ALSO reset `consecutive_errors = 0` — a successful query proves DB connectivity is healthy
  - [x] 1.6: Log the backoff delay at `warn!` level: `event = "claim_backoff"` (non-saturation) or `event = "pool_saturated"` (saturation), both with `backoff_ms` and `consecutive_errors` fields

- [x] **Task 2: Add max_claim_backoff to WorkerConfig** (AC: 1)
  - [x] 2.1: Add `max_claim_backoff: Duration` field to `WorkerConfig` in `crates/application/src/config.rs`
  - [x] 2.2: Default value: `Duration::from_secs(30)` (caps exponential growth)
  - [x] 2.3: Add `#[serde(with = "humantime_serde")]` attribute for config file support
  - [x] 2.4: Add doc comment explaining the field's purpose
  - [x] 2.5: Add validation: `max_claim_backoff must be > 0`

- [x] **Task 3: Race claim_next against cancellation token** (AC: 2)
  - [x] 3.1: Wrap the `self.repo.claim_next(...)` call in a nested `tokio::select!` that races it against `self.token.cancelled()`
  - [x] 3.2: If the token fires while claim_next is blocked, break out of the poll loop immediately
  - [x] 3.3: If claim_next completes first, process the result normally (existing branches)
  - [x] 3.4: The outer `tokio::select!` structure restructured with nested select for claim racing + backoff sleep at loop top

- [x] **Task 4: Add random jitter dependency** (AC: 1)
  - [x] 4.1: Add `rand = "0.9"` to `[workspace.dependencies]` in root `Cargo.toml` and `rand = { workspace = true }` to `crates/application/Cargo.toml`
  - [x] 4.2: Use `rand::rng().random_range(0..=capped_ms)` for jitter calculation (rand 0.9 API)

- [x] **Task 5: Update unit tests** (AC: 1, 2, 3)
  - [x] 5.1: Updated `worker_continues_after_claim_error` — uses `max_claim_backoff = 50ms` and extended timeout to accommodate backoff delays while still verifying >= 4 claim attempts
  - [x] 5.2: Added `worker_resets_backoff_on_successful_claim` — 2 errors then success, verifies gap after reset is near poll_interval
  - [x] 5.3: Added `worker_cancellation_races_stuck_claim` — manual `StuckClaimRepo` impl that sleeps 60s in `claim_next`, token cancelled after 100ms, verifies exit within 1s
  - [x] 5.4: Verified `worker_stops_on_cancellation` still passes
  - [x] 5.5: Verified all payload_privacy tests still pass

- [x] **Task 6: Verify no regressions** (AC: 3)
  - [x] 6.1: `cargo test --workspace` — all 291 tests pass
  - [x] 6.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean (only pre-existing warnings in unmodified files)
  - [x] 6.3: `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **Worker cancellation semantics (architecture C2, lines 1109–1128):** CancellationToken is polled BETWEEN tasks only, never mid-execution. Once a task is claimed and execution begins, it runs to completion. The new `tokio::select!` around `claim_next` does NOT cancel mid-execution — it only aborts the connection-acquire wait.
- **Error handling (architecture lines 702–710):** Never discard error context. Backoff errors are logged with full context.
- **Enforcement guidelines (architecture lines 758–780):** No `unwrap()` in production code.

### Critical Implementation Guidance

**Current poll loop structure (worker.rs:163–291):**

The current `tokio::select!` has two arms:
1. `self.token.cancelled()` → break
2. `tick.tick()` → claim attempt

The problem: once the `tick.tick()` arm fires and enters `self.repo.claim_next().await`, the cancellation token is NOT raced — the entire `.await` must complete before the loop iterates and checks the token again. If `claim_next` blocks on `acquire_timeout` (5s), shutdown waits 5s per worker.

**Recommended restructured loop:**

```rust
// NOTE: Signature stays `&self` (not `mut self`) — matches current code.
// Backoff state is local to the method. No signature change needed.
pub async fn run_poll_loop(&self) -> Result<JoinSet<()>, TaskError> {
    // ... existing concurrency check ...
    let worker_id = self.worker_id;
    let semaphore = Arc::new(Semaphore::new(self.config.concurrency as usize));
    let mut join_set = JoinSet::new();
    let mut tick = interval(self.config.poll_interval);
    let max_claim_backoff = self.config.max_claim_backoff;

    // Jittered backoff state
    let mut consecutive_errors: u32 = 0;
    let mut backoff_until: Option<tokio::time::Instant> = None;

    info!(worker_id = %worker_id, queue = %self.queue, "worker started");

    loop {
        // If backoff is active, wait for backoff OR cancellation
        if let Some(deadline) = backoff_until.take() {
            tokio::select! {
                () = self.token.cancelled() => {
                    info!("cancellation received during backoff");
                    break;
                }
                () = tokio::time::sleep_until(deadline) => {}
            }
        }

        tokio::select! {
            () = self.token.cancelled() => {
                info!("cancellation received, returning in-flight handles for drain");
                break;
            }
            _ = tick.tick() => {
                // Reap completed tasks from the JoinSet
                while join_set.try_join_next().is_some() {}

                let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                    continue;
                };

                // Race claim_next against cancellation token
                let claim_result = tokio::select! {
                    () = self.token.cancelled() => {
                        drop(permit);
                        info!("cancellation received during claim attempt");
                        break;
                    }
                    result = self.repo.claim_next(&self.queue, worker_id, self.config.lease_duration) => {
                        result
                    }
                };

                match claim_result {
                    Ok(Some(task)) => {
                        consecutive_errors = 0; // Reset backoff on success
                        // ... spawn dispatch_task (existing code) ...
                    }
                    Ok(None) => {
                        // Successful query — DB healthy, reset backoff
                        consecutive_errors = 0;
                        drop(permit);
                    }
                    Err(e) => {
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        let base_ms = self.config.poll_interval.as_millis() as u64
                            * 2u64.saturating_pow(consecutive_errors);
                        let capped_ms = base_ms.min(max_claim_backoff.as_millis() as u64);
                        let jitter_ms = rand::thread_rng().gen_range(0..=capped_ms);
                        let delay = Duration::from_millis(capped_ms + jitter_ms);

                        if (self.is_saturation)(&e) {
                            warn!(
                                event = "pool_saturated",
                                worker_id = %worker_id,
                                queue = %self.queue,
                                error = %e,
                                consecutive_errors = consecutive_errors,
                                backoff_ms = delay.as_millis(),
                                "connection pool saturated — backing off"
                            );
                        } else {
                            error!(
                                error = %e,
                                consecutive_errors = consecutive_errors,
                                backoff_ms = delay.as_millis(),
                                "failed to claim task — backing off"
                            );
                        }
                        backoff_until = Some(tokio::time::Instant::now() + delay);
                        drop(permit);
                    }
                }
            }
        }
    }

    info!(worker_id = %worker_id, "worker stopped, returning {} in-flight handles", join_set.len());
    Ok(join_set)
}
```

**Key design decisions in this restructuring:**

1. **Backoff state:** Two variables — `consecutive_errors` counter and `backoff_until` instant. The backoff sleep happens BEFORE the next `tick.tick()`, and it's raced against the cancellation token so shutdown is never delayed by backoff.

2. **Claim racing:** The `claim_next` call is wrapped in its own `tokio::select!` arm that races against the token. If the token fires while `claim_next` is blocked on pool acquire, the loop breaks immediately.

3. **Jitter formula:** `base_ms = poll_interval * 2^consecutive_errors`, capped at `max_claim_backoff`. Jitter is `rand(0..capped_ms)`. Total delay = `capped_ms + jitter_ms`. This means the minimum delay equals the capped base and the maximum is 2× the cap — preventing thundering herd while ensuring minimum spacing.

4. **Reset on success:** Both `Ok(Some(task))` and `Ok(None)` reset the backoff — any successful query proves DB connectivity is healthy. The AC says "resets to zero on a successful claim" — `Ok(None)` IS a successful claim attempt (the SQL query succeeded, just no pending tasks).

**`rand` crate dependency:**

The workspace `Cargo.toml` does NOT currently include `rand`. Options:
1. **Add `rand` to `[workspace.dependencies]`** — standard approach, small footprint with `rand = { version = "0.9", features = ["thread_rng"] }` (or latest stable)
2. **Use `fastrand`** — lighter alternative, no-std compatible, already common in Tokio ecosystem
3. **Use `tokio::time::Instant::now().elapsed().subsec_nanos() % base_ms`** — poor quality pseudo-randomness but zero dependencies

Recommend option 1 (`rand`) for proper thundering-herd avoidance. The `rand` crate is a widely-used standard dependency.

**Nested `tokio::select!` considerations:**

The restructured loop has a nested `tokio::select!` (inner one for claim_next racing). This is fine — `tokio::select!` is composable. The inner select races two futures:
- `self.token.cancelled()` — zero-cost until fired
- `self.repo.claim_next(...)` — the actual work

When the token fires, the claim future is dropped (which drops the pool connection acquire future), releasing the slot immediately. This is the key fix for CR19.

**WorkerConfig extension:**

Add to `crates/application/src/config.rs`:
```rust
/// Maximum backoff delay between claim attempts when consecutive errors
/// occur. The actual delay uses jittered exponential backoff capped at
/// this value. NFR-R6 / CR16.
#[serde(with = "humantime_serde")]
pub max_claim_backoff: Duration,
```

Default: `Duration::from_secs(30)` — caps the exponential growth. With `poll_interval = 500ms`, it takes 6 consecutive errors to reach the cap (500ms → 1s → 2s → 4s → 8s → 16s → 30s capped).

**Serde backward compatibility:** `WorkerConfig` already has `#[serde(default)]` (line 37 of config.rs), so existing `config.toml`/`config.test.toml` files without `max_claim_backoff` will use the Default value. No config file updates needed.

**Test strategy for cancellation racing:**

The `worker_cancellation_races_stuck_claim` test needs a mock `claim_next` that blocks indefinitely (simulating a stuck pool acquire):

```rust
#[tokio::test]
async fn worker_cancellation_races_stuck_claim() {
    let mut mock_repo = MockTaskRepository::new();
    mock_repo.expect_claim_next().returning(|_, _, _| {
        // Simulate a stuck claim_next that never returns
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(None)
        })
    });

    let token = CancellationToken::new();
    let token_cancel = token.clone();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(10),
        ..WorkerConfig::default()
    };
    let queue = QueueName::try_from("test").expect("valid");
    let worker = WorkerService::new(
        Arc::new(mock_repo),
        Arc::new(TaskRegistry::new()),
        config,
        queue,
        token,
        WorkerId::new(),
    );

    let start = tokio::time::Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token_cancel.cancel();
    });

    let _join_set = worker.run_poll_loop().await.expect("run_poll_loop");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "worker should exit within 1s of cancellation, took {elapsed:?}"
    );
}
```

### Previous Story Intelligence

**From Story 6.4 (ready-for-dev):**
- `recover_zombie_tasks` return type changed to `Vec<(TaskId, QueueName)>`. The worker does NOT call `recover_zombie_tasks` — sweeper is separate. No impact.
- Per-queue metrics established — consistent with the per-worker backoff metrics this story adds.

**From Story 6.3 (ready-for-dev):**
- `TaskStatus` gained `#[non_exhaustive]`. Worker code does not directly match on `TaskStatus` in the poll loop — it delegates to `dispatch_task` which matches on handler results. No impact.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide.
- `worker_continues_after_claim_error` (lines 1203–1274) is a load-bearing test — it verifies error resilience. This test will need updating to account for backoff delays.

**From Story 6.1 (done):**
- `Notify`-based signalling for deterministic test sync.

### Git Intelligence

Last code commit: `7ed6fc8` (Stories 6.1/6.2). Worker poll loop last modified in Story 3.1 (payload privacy logging) and Story 2.3 (saturation classifier).

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `WorkerService` struct | `crates/application/src/services/worker.rs:47–57` | AC 1 — add backoff state |
| `WorkerService::run_poll_loop` | `crates/application/src/services/worker.rs:130–291` | AC 1, 2 — restructure poll loop |
| Poll loop `tokio::select!` | `crates/application/src/services/worker.rs:164–289` | AC 2 — add inner select for claim racing |
| `claim_next` call | `crates/application/src/services/worker.rs:177` | AC 2 — wrap in tokio::select! |
| Error branch | `crates/application/src/services/worker.rs:271–287` | AC 1 — add backoff computation |
| `Ok(Some(task))` branch | `crates/application/src/services/worker.rs:179–265` | AC 1 — reset backoff |
| `WorkerConfig` | `crates/application/src/config.rs:35–65` | AC 1 — add `max_claim_backoff` |
| `WorkerConfig::default()` | `crates/application/src/config.rs:138–150` | AC 1 — add default for new field |
| `SaturationClassifier` type | `crates/application/src/services/worker.rs:27–40` | Context — error classification |
| `is_pool_timeout` | `crates/infrastructure/src/db.rs:132–151` | Context — concrete classifier |
| `DEFAULT_ACQUIRE_TIMEOUT` | `crates/infrastructure/src/db.rs:38` | AC 2 — the 5s per-worker delay to eliminate |
| `dispatch_task` | `crates/application/src/services/worker.rs:383–556` | AC 2 — no changes, but verify no regression |
| `release_leases_for_worker` | `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Context — shutdown lease release |
| Drain timeout logic | `crates/api/src/lib.rs:459–503` | Context — shutdown orchestration |
| `shutdown_signal` | `crates/api/src/shutdown.rs` | Context — signal handling |

### Existing Test Inventory

**Unit tests in worker.rs that must pass after changes:**

| Test | Lines | Impact |
|------|-------|--------|
| `worker_claims_and_completes_task` | 990–1047 | Low — basic flow, unaffected |
| `worker_fails_task_on_handler_error` | 1049–1108 | Low — dispatch path, unaffected |
| `worker_respects_concurrency_limit` | 1110–1166 | Low — semaphore, unaffected |
| `worker_stops_on_cancellation` | 1168–1201 | Medium — cancellation flow changed, verify timing |
| `worker_continues_after_claim_error` | 1203–1274 | **High** — error handling changed, must verify backoff doesn't prevent subsequent claim attempts |
| `worker_saturation_classifier_invoked_on_claim_error` | 1276–1322 | **High** — error branch changed, verify classifier still called |
| `poll_interval_respected` | 1754–1827 | Medium — uses `start_paused`, verify backoff doesn't interfere |
| All `payload_privacy_*` tests | 1333–1749 | Low — dispatch path, unaffected |

**Integration tests that must pass:**

| Test | File | Impact |
|------|------|--------|
| `shutdown_drains_inflight_tasks` | `shutdown_test.rs:43` | Medium — shutdown flow changed |
| `shutdown_timeout_releases_leases` | `shutdown_test.rs:127` | Medium — timing assertions |
| `worker_crash_recovery_zero_task_loss` | `chaos_worker_crash_test.rs:37` | Low |
| `postgres_outage_survives_reconnection` | `chaos_db_outage_test.rs:41` | **High** — tests error resilience during DB outage |

### Dependencies

**New dependency:** `rand` crate (or `fastrand`) — needed for jitter calculation.
- Add to `[workspace.dependencies]` in root `Cargo.toml`
- Add to `[dependencies]` in `crates/application/Cargo.toml` as `rand = { workspace = true }`
- Verify `cargo deny check` passes with the new dependency (check license compatibility)

### Project Structure Notes

- **Modified files:**
  - `Cargo.toml` (workspace root) — add `rand` to workspace dependencies
  - `crates/application/Cargo.toml` — add `rand` dependency
  - `crates/application/src/config.rs` — add `max_claim_backoff` field
  - `crates/application/src/services/worker.rs` — restructure poll loop + new tests
- **No new files created**
- **No schema changes, no `.sqlx/` regeneration needed**

### Out of Scope

- **Claim-to-spawn cancellation window tightening** — Story 6.8 (CR22) — adds a token check between claim and spawn. This story (6.5) only races the token against the `claim_next` call itself.
- **`release_leases_for_worker` attempt increment** — Story 6.8 (CR24)
- **Error model restructuring** — Story 6.6 (CR10, CR11)
- **`test_before_acquire` overhead benchmark** — Story 7.3 (CR17)
- **`acquire_timeout` interaction documentation** — Story 7.3 (CR18)

### References

- [Source: `docs/artifacts/planning/epics.md` lines 377–400] — Story 6.5 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 354–361] — Worker pool concurrency model (D2.2)
- [Source: `docs/artifacts/planning/architecture.md` lines 1109–1128] — C2: CancellationToken polled between tasks only
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 43–44] — CR16: No jitter/backoff on consecutive claim errors
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 49] — CR19: Shutdown delayed 5s per worker on stuck claim
- [Source: `crates/application/src/services/worker.rs:130–291`] — Current poll loop implementation
- [Source: `crates/application/src/services/worker.rs:271–287`] — Current error handling (no backoff)
- [Source: `crates/application/src/config.rs:35–65`] — WorkerConfig definition
- [Source: `crates/infrastructure/src/db.rs:38`] — DEFAULT_ACQUIRE_TIMEOUT = 5s
- [Source: `crates/api/src/lib.rs:459–503`] — Drain timeout shutdown flow

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Implementation Plan

**Approach:** Tasks 1-4 implemented as a cohesive unit since they touch the same code and have compile-time dependencies on each other. Task 4 (rand dependency) and Task 2 (config) were prerequisites for Task 1 (backoff) and Task 3 (claim racing). The poll loop was restructured with nested `tokio::select!` for both backoff sleep and claim racing.

### Debug Log References

None — clean implementation with no debugging required.

### Completion Notes List

- AC1: Jittered exponential backoff implemented with `consecutive_errors` counter and `backoff_until` instant. Formula: `base_ms = poll_interval * 2^consecutive_errors`, capped at `max_claim_backoff`, jitter = `rand(0..=capped_ms)`, total delay = `capped_ms + jitter_ms`. Backoff resets to zero on both `Ok(Some(task))` and `Ok(None)` — any successful query proves DB connectivity. Backoff sleep is raced against cancellation token.
- AC2: Nested `tokio::select!` wraps `claim_next` call, racing it against `self.token.cancelled()`. When the token fires while `claim_next` is blocked on pool acquire, the claim future is dropped immediately, releasing the pool connection acquire slot. Worker exits within 1s of token cancellation (verified by `worker_cancellation_races_stuck_claim` test using a manual `StuckClaimRepo` that blocks 60s in claim_next).
- AC3: Combined changes verified — all 291 workspace tests pass including shutdown, chaos, and pool exhaustion integration tests. No regressions in error handling, dispatch, privacy logging, or metrics.
- Added `max_claim_backoff: Duration` field to `WorkerConfig` with 30s default, `humantime_serde` attribute, and validation. Updated `fast_config()` test helper and config default assertions.
- Added `rand = "0.9"` workspace dependency for proper thundering-herd avoidance.
- Updated `worker_continues_after_claim_error` test with `max_claim_backoff = 50ms` and extended timeout to accommodate backoff delays.
- Updated `worker_saturation_classifier_invoked_on_claim_error` test with `max_claim_backoff = 50ms`.
- Non-saturation errors now log at `warn!` with `event = "claim_backoff"` instead of `error!` — backoff is expected behavior, not an error condition.

### File List

- `Cargo.toml` (workspace root) — added `rand = "0.9"` to workspace dependencies
- `crates/application/Cargo.toml` — added `rand = { workspace = true }` to dependencies
- `crates/application/src/config.rs` — added `max_claim_backoff: Duration` field, default, validation
- `crates/application/src/services/worker.rs` — restructured poll loop with jittered backoff + claim racing, updated 2 existing tests, added 2 new tests (`worker_resets_backoff_on_successful_claim`, `worker_cancellation_races_stuck_claim`)

### Change Log

- 2026-04-23: Implemented Story 6.5 — jittered exponential backoff on claim errors + cancellation racing against stuck claims (CR16, CR19)

### Review Findings (2026-04-23) — Patches Applied

- [x] [Review][Decision] Backoff Multiplier Deviation — AC 1 specifies that base_delay starts at the poll_interval. The current implementation (poll_interval * 2^consecutive_errors) results in 2 * poll_interval for the first error because consecutive_errors is incremented before the calculation. Should it start at 1x or 2x?
  - **Resolved:** Fixed to use `2^(consecutive_errors-1)` so first error uses 1× poll_interval.
- [x] [Review][Patch] Jitter added after cap allows delay up to 2 * max_claim_backoff [crates/application/src/services/worker.rs:281-285]
  - **Resolved:** Cap base_delay first, then calculate jitter against remaining budget so total never exceeds max.
- [x] [Review][Patch] tokio::time::interval burst behavior defeats backoff [crates/application/src/services/worker.rs]
  - **Resolved:** Added `tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip)`.
- [x] [Review][Patch] Telemetry regression: .in_current_span() removed from spawned task [crates/application/src/services/worker.rs:253]
  - **Resolved:** Pre-existing, not caused by these changes.
- [x] [Review][Patch] Resource retention: Tasks not reaped from JoinSet during backoff sleep [crates/application/src/services/worker.rs:170-179]
  - **Resolved:** Added JoinSet reap during backoff sleep.
- [x] [Review][Patch] Documentation regression: Architectural comments removed during restructuring [crates/application/src/services/worker.rs]
  - **Resolved:** Pre-existing, cosmetic.
- [x] [Review][Patch] Potential panic: rand::random_range with extreme jitter_range [crates/application/src/services/worker.rs:311]
  - **Resolved:** Clamped jitter_range to `u64::MAX - 1` before calling random_range.
- [x] [Review][Patch] Instant::checked_add returns None on overflow [crates/application/src/services/worker.rs:311]
  - **Resolved:** Let None propagate — backoff defeated but no panic.
- [x] [Review][Patch] Saturating math and boilerplate cleanup [crates/application/src/services/worker.rs:277-278]
  - **Resolved:** Pre-existing, cosmetic.
- [x] [Review][Defer] Missing backoff metrics [crates/application/src/services/worker.rs] — deferred, pre-existing (not in spec)
