# Story 2.3: Postgres Auto-Reconnection

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a platform engineer,
I want the engine to automatically reconnect to PostgreSQL after a connection loss,
so that transient database outages do not cause permanent task loss or require manual restart.

## Acceptance Criteria

1. **`create_pool()` helper hardened for transient failure recovery (FR16):**
   - In `crates/infrastructure/src/db.rs`, `create_pool()` continues to accept a `&DatabaseConfig` and return `Result<PgPool, TaskError>`.
   - `PgPoolOptions` is configured with **all** of the following (documented defaults, each defined as a `pub const` in `db.rs`):
     - `.max_connections(max_connections)` — resolved from `DatabaseConfig::max_connections` with `0 → DEFAULT_MAX_CONNECTIONS` (already in place).
     - `.acquire_timeout(DEFAULT_ACQUIRE_TIMEOUT)` — remains `Duration::from_secs(5)` (already in place).
     - `.min_connections(0)` — let the pool shed idle connections during outages; new `DEFAULT_MIN_CONNECTIONS: u32 = 0` const.
     - `.idle_timeout(Some(DEFAULT_IDLE_TIMEOUT))` — `Duration::from_secs(300)` (5 min). Idle connections are recycled so stale TCP sessions do not silently linger across outages.
     - `.max_lifetime(Some(DEFAULT_MAX_LIFETIME))` — `Duration::from_secs(1800)` (30 min). Upper bound regardless of idleness; protects against slow TCP half-open states and DB-side connection caps.
     - `.test_before_acquire(true)` — SQLx pings every checked-out connection with a lightweight round-trip; stale connections (e.g. after a Postgres restart) are dropped and replaced transparently.
   - Each constant has a short doc comment citing the ADR / PRD / architecture source.
   - Existing behaviour (translation of `sqlx::Error` → `TaskError::Storage` via `PostgresAdapterError`) is preserved; no new panics, no new `unwrap()` / `expect()` in `src/`.
   - The inline unit test module in `db.rs` gains assertions that the new constants are finite and non-zero (same style as the existing `default_acquire_timeout_is_finite` test).

2. **Worker poll loop survives connection loss without panics (FR16):**
   - `WorkerService::run_poll_loop` (in `crates/application/src/services/worker.rs`) must continue to log-and-continue when `claim_next` returns `Err`. This is **already** the behaviour at `worker.rs:147-150` (`error!(error = %e, "failed to claim task");` → `drop(permit);`). The story verifies and documents this with tests; it does **not** rewrite the loop.
   - No additional sleep-on-error backoff is added in this story — the existing `tokio::time::interval` tick provides the retry cadence (default 500ms). Jitter/backoff on consecutive DB errors is explicitly **out of scope** (deferred; same "no jitter on retry backoff" item tracked in `deferred-work.md` for Epic 2).
   - A new **unit test** in `worker.rs` (`worker_continues_after_claim_error`): a `MockTaskRepository` returns `Err(TaskError::Storage{..})` on the first N `claim_next` calls and `Ok(Some(task))` afterwards; the worker must process the task after the transient error window without panicking. Verifies log-and-retry.

3. **Sweeper survives connection loss without panics (FR16):**
   - `SweeperService::start` already logs and continues on `recover_zombie_tasks` errors (`sweeper.rs:75-85`; covered by the existing `sweeper_continues_on_error` unit test). No code change required.
   - Confirm this behaviour is exercised for the `TaskError::Storage` variant specifically and document it in Dev Notes. No new sweeper test is required; the existing `sweeper_continues_on_error` test already uses `TaskError::Storage`.

4. **Pool saturation surfaces a warning log (NFR-R6):**
   - When the worker poll loop observes a claim error whose root cause is pool-acquire timeout, it must emit a **`warn!`** level log (not `error!`) with `event = "pool_saturated"`, `worker_id`, and `queue` fields. All other errors continue to log at `error!` as today.
   - **Detection:** walk the `std::error::Error::source()` chain of the returned `TaskError::Storage` until a `sqlx::Error` is found; match `sqlx::Error::PoolTimedOut`. Implement this as a small private helper in `worker.rs` — `fn is_pool_saturation(err: &TaskError) -> bool` — guarded by a unit test using a synthetic error chain.
   - The `pool_wait_queue_depth` **metric** mentioned in NFR-R6 is an OTel instrument; emission is deferred to **Epic 3 / Story 3.2** (OTel metrics). This story delivers the log-level half of NFR-R6 only. Record this deferral in `deferred-work.md`.
   - No silent task loss: the `Err` branch still drops the permit and continues; tasks remain in `Pending` and are re-claimed on the next successful poll.

5. **`PostgresAdapterError` preserves the `sqlx::Error` source chain across `TaskError::Storage`:**
   - Verify (no code change expected) that `PostgresAdapterError` → `TaskError::Storage { source }` keeps the underlying `sqlx::Error` reachable via `e.source()` walks. Story 1A.2's review resolved the `#[source]` wiring; Task 4's detection helper depends on it.
   - Add a **unit test** in `crates/infrastructure/src/error.rs` (or a new test module) that builds a `sqlx::Error::PoolTimedOut` → `PostgresAdapterError` → `TaskError` and asserts `TaskError`'s source chain contains the original `PoolTimedOut`.
   - If the source chain is broken, **fix the `From` impl** rather than working around it in `worker.rs`.

6. **Integration test: tasks survive a Postgres outage (TEA P0-CHAOS / Architecture line 419):**
   - Added as `postgres_outage_survives_reconnection` in a **new file** `crates/api/tests/db_outage_integration_test.rs` (naming convention consistent with `worker_integration_test.rs`).
   - This test **does NOT** use the shared `TEST_DB OnceCell` from `tests/common/mod.rs`. Per architecture lines 952-955, chaos-style tests that kill connections must spin up their own isolated Postgres container. The helper `boot_isolated_test_db()` (new, in this test file or in `tests/common/mod.rs` gated behind a new helper name — implementer's choice) creates a fresh `ContainerAsync<Postgres>` + `PgPool` for this test only.
   - Test flow:
     1. Start isolated Postgres container.
     2. Build `IronDefer` with `poll_interval = 100ms`, `sweeper_interval = 500ms`, `concurrency = 2`, `shutdown_timeout = 5s`.
     3. Register a handler for a fast `CountingTask` kind that increments a shared `AtomicUsize`.
     4. Enqueue 20 tasks.
     5. Spawn `engine.start(token.clone())` on a background task.
     6. After ~2 seconds (some tasks processed), **stop the container** via `container.stop().await`.
     7. Sleep 3 seconds (outage window).
     8. **Start the container again** (`container.start().await`). Since testcontainers re-exposes the same mapped port on restart, the existing pool must reconnect on its own.
     9. Wait up to **30 seconds** for the `AtomicUsize` to reach 20 (polling every 250ms).
     10. Cancel the token, await shutdown.
   - Assertions:
     - All 20 tasks reach `Completed` (verified via raw SQL `SELECT COUNT(*) FROM tasks WHERE status = 'completed'` = 20).
     - Zero tasks remain in `Running` (`SELECT COUNT(*) FROM tasks WHERE status = 'running'` = 0).
     - At least one `warn!`/`error!` log was emitted during the outage window (captured via `tracing_test` subscriber or equivalent — if the subscriber setup is too heavy, assert only the final row counts and document why log capture is deferred).
   - The test is marked `#[tokio::test]` and **not** `#[ignore]` — it is a mandatory CI scenario per PRD line 419.
   - **Timeout guard:** wrap the "wait for completion" loop in `tokio::time::timeout(Duration::from_secs(45), ...)` so a broken reconnection fails fast instead of hanging CI.

7. **`TaskError::Storage` payload scrubbing is preserved:**
   - No `payload` / `kind` / `task_id` content is logged as part of the new warning emission (per FR38 — payload privacy by default). The pool-saturation warn log carries only `event`, `worker_id`, `queue`, and the underlying error's `Display` string (which, for `PoolTimedOut`, contains no task data).
   - This is a verification step, not a code change. Document in Dev Notes.

8. **Documentation — runbook pointer in `README.md` or `docs/guidelines/`:**
   - Add a short **"Postgres reconnection behaviour"** subsection to one of: `docs/guidelines/` (preferred), `README.md` (acceptable), or a new `docs/operations.md`. Content:
     - SQLx built-in pool reconnection is the primary mechanism.
     - Default pool constants from this story (`DEFAULT_IDLE_TIMEOUT`, `DEFAULT_MAX_LIFETIME`, `DEFAULT_ACQUIRE_TIMEOUT`, `DEFAULT_MAX_CONNECTIONS`, `test_before_acquire = true`).
     - Tuning guidance: if operators see frequent `pool_saturated` warnings, increase `max_connections` or investigate downstream slow queries.
     - Reference to Story 3.2 for the `pool_wait_queue_depth` metric.
   - Two paragraphs is sufficient; detailed operator docs are Epic 5 (production readiness).

9. **Quality gates pass:**
   - `cargo fmt --check`
   - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
   - `SQLX_OFFLINE=true cargo test --workspace`
   - `cargo deny check bans`
   - `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` must print nothing (rustls-only preserved).

## Tasks / Subtasks

- [x] **Task 1: Harden `create_pool()` with resilience options** (AC 1)
  - [x] In `crates/infrastructure/src/db.rs`, add new public constants with doc comments:
    - [x] `pub const DEFAULT_MIN_CONNECTIONS: u32 = 0;`
    - [x] `pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);`
    - [x] `pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(1800);`
  - [x] Extend the `PgPoolOptions::new()` builder chain in `create_pool()` with `.min_connections(..)`, `.idle_timeout(Some(..))`, `.max_lifetime(Some(..))`, `.test_before_acquire(true)` — keep the existing `max_connections` + `acquire_timeout` calls unchanged.
  - [x] Extend the inline unit test module: `default_idle_timeout_is_finite`, `default_max_lifetime_is_finite`, `default_min_connections_is_zero` (style parity with existing `default_acquire_timeout_is_finite`).
  - [x] Run `cargo test -p iron-defer-infrastructure`.

- [x] **Task 2: Verify `TaskError::Storage` source chain + pool-saturation helper** (AC 4, AC 5)
  - [x] In `crates/infrastructure/src/error.rs` (or a new test module), add a unit test that builds a `sqlx::Error::PoolTimedOut`, wraps into `PostgresAdapterError`, converts to `TaskError`, and asserts `std::error::Error::source()` chain walking from the `TaskError` reaches the original `sqlx::Error::PoolTimedOut` variant. — `task_error_storage_preserves_sqlx_pool_timeout_source` in `crates/infrastructure/src/error.rs`.
  - [x] Helper placed in **infrastructure** as `iron_defer_infrastructure::is_pool_timeout(&TaskError) -> bool`. Rationale: `application/Cargo.toml` does NOT have `sqlx` as a dep, and architecture lines 925–934 disallow adding it. The worker (in `application`) accepts an injected classifier closure (`SaturationClassifier`); the wiring in `crates/api/src/lib.rs` passes `is_pool_timeout` into `WorkerService::with_saturation_classifier`.
  - [x] Added unit tests: `is_pool_timeout_detects_pool_timed_out`, `is_pool_timeout_rejects_other_sqlx_errors`, `is_pool_timeout_rejects_non_storage_variants` in `db.rs`.

- [x] **Task 3: Worker pool-saturation warn log** (AC 4, AC 7)
  - [x] Branch on injected `is_saturation(&e)` classifier in the poll-loop error arm:
    - [x] Saturated → `warn!(event = "pool_saturated", worker_id, queue, error, "postgres connection pool saturated — task claim deferred");`
    - [x] Otherwise → existing `error!(error = %e, "failed to claim task");`.
  - [x] Permit release and tick control flow unchanged.
  - [x] **Payload privacy:** neither log line carries task/payload fields.
  - [x] `cargo test -p iron-defer-application` — 22 passed.

- [x] **Task 4: Worker unit test — continues after transient DB error** (AC 2)
  - [x] `worker_continues_after_claim_error` in `worker.rs`: returns `Err(TaskError::Storage{..})` for first 3 claims, then success — asserts `run()` processes the successful task and does not exit on errors.
  - [x] Bonus: `worker_saturation_classifier_invoked_on_claim_error` verifies the classifier is called on storage errors (AC 4).

- [x] **Task 5: Integration test — Postgres outage + reconnection** (AC 6)
  - [x] New file `crates/api/tests/db_outage_integration_test.rs`.
  - [x] `boot_isolated_test_db()` spins a fresh `ContainerAsync<Postgres>`, runs migrations via `IronDefer::migrator()`, returns `(pool, container, url)`. Uses `create_pool()` so the hardened pool options are exercised by the chaos path. Shared `TEST_DB OnceCell` deliberately NOT used.
  - [x] `CountingTask` uses `static COUNTER: AtomicUsize`.
  - [x] `postgres_outage_survives_reconnection` follows the AC 6 flow end-to-end. After container restart, a **fresh verification pool** is built against the post-restart URL for the raw-SQL assertions (the engine's long-lived pool may hold lingering broken connections; a new pool acts as the "external operator" view).
  - [x] `tokio::time::timeout(Duration::from_secs(45), ...)` wraps the completion loop.
  - [x] Raw SQL: `SELECT COUNT(*) WHERE status = 'completed'` = 20 and `status = 'running'` = 0.
  - [x] Passes reliably: 3 consecutive runs, ~13–17s each.

- [x] **Task 6: Documentation — reconnection runbook** (AC 8)
  - [x] New file `docs/guidelines/postgres-reconnection.md` — constants table, `test_before_acquire` rationale, saturation log description, operator tuning tips, Story 3.2 reference, and explicit "what iron-defer does NOT do" block.

- [x] **Task 7: `deferred-work.md` entry for the OTel metric half of NFR-R6** (AC 4)
  - [x] Added "Deferred from: implementation of 2-3-postgres-auto-reconnection (2026-04-15)" section with 4 entries: OTel gauge deferral, jitter/backoff, `test_before_acquire` overhead, embedded-mode opt-in.

- [x] **Task 8: Quality gates** (AC 9)
  - [x] `cargo fmt --check` — clean.
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean.
  - [x] Unit tests per crate: `iron-defer-application` 22/22, `iron-defer-infrastructure` 28/28, `iron-defer` (lib) 5/5.
  - [x] `SQLX_OFFLINE=true cargo test -p iron-defer --test db_outage_integration_test` — 1/1 passing (verified stable across 3 consecutive runs).
  - [x] `cargo deny check bans` — `bans ok`.
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty.
  - [ ] `SQLX_OFFLINE=true cargo test --workspace` — **pre-existing flakiness**: `integration_test` and `worker_integration_test` (shared `TEST_DB OnceCell`) intermittently fail with `PoolTimedOut` during `IronDefer::build()`. **Verified NOT a regression**: reverting all Story 2.3 changes (via `git stash`) reproduces the same failures on `main`. Known issue per Stories 1A.2 + 2.2 deferred-work entries about shared-pool saturation. The marquee test for this story (`db_outage_integration_test`) uses an isolated container and is green.

### Review Findings

- [x] [Review][Patch] Broaden `is_pool_timeout` to cover outage connectivity errors [crates/infrastructure/src/db.rs:106-117] — Also treat `sqlx::Error::Io`, `sqlx::Error::PoolClosed`, and `sqlx::Error::Database` with SQLSTATE class `08` (connection exception) as saturation/outage signals. Rename to reflect widened contract or keep the name and document it. Update unit tests accordingly. Motivation: during a real outage most errors are not `PoolTimedOut`, so the strict match contradicts NFR-R6's warn-for-outage spirit.
- [x] [Review][Patch] Chaos test silently passes when Docker is unavailable [crates/api/tests/db_outage_integration_test.rs:108-111] — `return` on `boot_isolated_test_db() == None` reports success; AC 6 mandates "not `#[ignore]` — mandatory CI scenario per PRD 419". Fix: `panic!` with a clear message (optionally honoring an `IRON_DEFER_SKIP_DOCKER_CHAOS=1` opt-out for local dev).
- [x] [Review][Patch] Engine-task panic during outage is swallowed [crates/api/tests/db_outage_integration_test.rs:196] — `let _ = tokio::time::timeout(..., engine_task).await;` discards `JoinError`; a panic in the poll loop during the chaos window fails invisibly. Fix: await the join and assert `is_ok()` (or unwrap) so panics propagate.
- [x] [Review][Patch] Chaos test does not assert a `warn!`/`error!` log fired during the outage window [crates/api/tests/db_outage_integration_test.rs] — AC 6 requires either log-capture assertion OR explicit Dev Notes / deferred-work rationale. Neither is present. Fix: add a Dev Notes paragraph documenting the deferral and a corresponding `deferred-work.md` entry (or add `tracing_test` capture if subscriber setup is tractable).
- [x] [Review][Patch] Port remap after container restart silently hangs the test for 45 s [crates/api/tests/db_outage_integration_test.rs:174-180] — The engine's pool is pinned to the pre-outage URL; when Docker remaps the host port the workers cannot reconnect and the test fails via timeout with a misleading "tasks did not complete" message. Fix: assert `restart_port == original_port` immediately after restart with a targeted failure message ("Docker reassigned the port — reconnection cannot be validated on this host").
- [x] [Review][Patch] Sweeper logs all storage errors as `error!` during outages — defeats NFR-R6 warn-level spirit [crates/application/src/services/sweeper.rs] — `SweeperService` has no saturation classifier; during the very outage scenario that Story 2.3 targets, the sweeper floods `error!` while the worker correctly warns. Fix: plumb the same `SaturationClassifier` into `SweeperService`, wire `is_pool_timeout` at `IronDefer::start()`, and branch the `warn!`/`error!` path identically to the worker.
- [x] [Review][Defer] `test_before_acquire(true)` ping budget can exhaust `acquire_timeout=5s` during active outage [crates/infrastructure/src/db.rs:87] — deferred, revisit in Epic 5 benchmarks.
- [x] [Review][Defer] `is_pool_timeout` has no cycle guard on `source()` chain [crates/infrastructure/src/db.rs:106-117] — deferred, hypothetical, no known offender in the dep graph.
- [x] [Review][Defer] Shutdown can be delayed up to 5 s per worker on a stuck `claim_next` during outage [crates/application/src/services/worker.rs:141-191] — deferred, existing design; `shutdown_timeout` absorbs it.
- [x] [Review][Defer] Classifier closure can panic the poll loop — no `catch_unwind` guard [crates/application/src/services/worker.rs:175] — deferred, all in-crate classifiers are non-panicking; revisit if a public hook lands.

## Dev Notes

### Architecture Compliance

- **Technical Constraints (lines 76-88):** SQLx is the sole Postgres driver, `runtime-tokio-rustls` feature, no OpenSSL. Pool options chosen in Task 1 must not introduce new TLS backends.
- **ADR-0005 (rustls-only):** Verified via `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — Task 8 gate.
- **Architecture line 488, 936-939 (public API surface):** `PgPool` remains the single `sqlx` type crossing the public boundary. The embedded library accepts a caller-provided `PgPool` — callers who construct their own pool are responsible for their own options. `create_pool()` is the **standalone-binary** helper; the hardened defaults apply to standalone mode. Document this asymmetry briefly in the runbook (Task 6).
- **Architecture lines 952-955 (chaos isolation):** The outage integration test MUST NOT use the shared `TEST_DB OnceCell`. Each chaos-style test spins up its own container. This is why Task 5 creates a local helper rather than adding to `tests/common/mod.rs`.
- **PRD line 374:** "iron-defer ... handles connection loss gracefully: workers back off and retry reconnection; the Sweeper resumes automatically when connectivity is restored." This story makes that promise real and testable.
- **PRD line 419:** "Postgres restart ... is a mandatory chaos scenario, not optional." Satisfied by AC 6.
- **FR16:** Covered by ACs 1–3 and AC 6.
- **NFR-R1 (zero task loss):** Covered by AC 6 integration test (20 tasks enqueued, 20 completed after outage).
- **NFR-R6 (pool saturation):** Log half delivered (AC 4); metric half deferred to Story 3.2 and recorded in `deferred-work.md` (Task 7).
- **FR38 (payload privacy):** Enforced in Task 3 — neither the `warn!` nor the existing `error!` line logs payload data (AC 7).
- **Architectural boundary (lines 925-934):** If Task 2's helper needs `sqlx::Error` downcast, prefer to implement the boolean in `infrastructure` rather than adding `sqlx` to `application/Cargo.toml`. Layer rule: `application` depends on `domain` only.

### Critical Design Decisions

**SQLx pool is the reconnection engine, not us.**
The entire point of ACs 1–3 is that SQLx's `PgPool` **already** reconnects transparently. Our job is to:
1. Configure the pool with resilience-appropriate options (`test_before_acquire`, `idle_timeout`, `max_lifetime`) so stale TCP connections are detected and replaced.
2. Ensure no code path panics on connection errors — workers and sweeper already log-and-continue.
3. Prove it with an integration test that stops/starts the Postgres container.
4. Surface pool saturation (the one failure mode SQLx does NOT auto-heal) as a warning log so operators can act.

Do **not** write a custom reconnection loop. Do **not** introduce a `watchdog` task that reconstructs the `PgPool` on error. Both would break the caller-provided-`PgPool` contract (Architecture line 488) and duplicate SQLx functionality.

**Why `test_before_acquire = true` is worth the extra round-trip.**
SQLx's docs call this "a small overhead" (~1 ms LAN). Without it, a worker could check out a half-open TCP connection from the pool after a Postgres restart, issue `claim_next`, fail, and discard the connection — repeat for every surviving pool entry. `test_before_acquire` short-circuits this by pinging once at checkout. For a 500ms poll interval, the overhead is negligible; for chaos recovery, it is the difference between reconnecting in one poll cycle vs. `max_connections` cycles.

**Why `min_connections = 0`.**
Eager pool warm-up conflicts with outage recovery: an idle pool with `min_connections > 0` will burn CPU trying to re-establish the floor while Postgres is down. `min_connections = 0` lets the pool go fully cold, eliminating the retry storm. Cold-start latency is masked by the first claim's `acquire_timeout` budget.

**Pool saturation detection — downcast strategy.**
Walking `std::error::Error::source()` is the idiomatic way to reach the underlying `sqlx::Error`. Story 1A.2's review resolved `TaskError::Storage { #[source] source: Box<dyn std::error::Error + Send + Sync> }`, so the chain is intact. If the downcast lives in `infrastructure` (per layer rules), export a single function — do not leak the full `sqlx::Error` type across the crate boundary.

### Previous Story Intelligence (from Story 2.2)

**Code patterns that MUST be followed (confirmed across 2.1 + 2.2):**

- `#[instrument(skip(self), fields(...), err)]` on every public async method. Payload never in fields.
- No `unwrap()` / `expect()` / `panic!()` in `src/` outside `#[cfg(test)]`. Map all errors to `TaskError`.
- Error source chains preserved — never discard context. (Task 2 verifies this explicitly.)
- Integration tests use the shared `OnceCell<Option<TestDb>>` testcontainers pattern from `tests/common/mod.rs` — **EXCEPT** chaos / outage tests, which each spin their own container per Architecture lines 952-955. Story 2.3's Task 5 follows the latter rule.
- Load-bearing tests verify via raw SQL, not just API return values.

**Key review findings from 2.2 that informed this story:**

- `release_leases_for_worker` does not bump `attempts` (deferred; pre-existing). Not relevant here.
- `shutdown_timeout_releases_leases` elapsed `< 5s` assertion tight under CI load (deferred). Informs the `timeout(45s)` guard in AC 6.
- Shared `OnceCell` pool saturation under parallel runs is a known test-infra issue. Task 5's isolated container avoids sharing entirely.

**Key types and locations (verified current as of 2026-04-14):**

- `create_pool` — `crates/infrastructure/src/db.rs:55-73`
- `PostgresAdapterError` → `TaskError::Storage` — `crates/infrastructure/src/error.rs` (confirm path; `From` impl lives there)
- `WorkerService::run_poll_loop` — `crates/application/src/services/worker.rs:100-157` (error branch at 147-150)
- `SweeperService::start` — `crates/application/src/services/sweeper.rs:75-85` (already log-and-continue; no change)
- `DatabaseConfig` — `crates/application/src/config.rs:22-29` (no new fields in this story; all defaults are constants in `db.rs`)
- `MIGRATOR` — `crates/infrastructure/src/db.rs:41` (used by Task 5's isolated container helper)
- `TEST_DB OnceCell` — `crates/api/tests/common/mod.rs` (must NOT be used by Task 5; reference only)

**Dependencies — no new crates required:**

- `sqlx` — already present; `PoolTimedOut` variant is part of `sqlx::Error` in current versions.
- `tokio` — `signal` feature already added in Story 2.2.
- `tracing` — `warn!`, `error!`, `instrument` already in use.
- `testcontainers` / `testcontainers-modules` — already dev-deps; support `container.stop().await` and `container.start().await`.

**No workspace Cargo.toml changes expected.** If Task 2's helper is placed in `application` and `sqlx` is not already an `application` dep, **prefer** exposing the boolean from `infrastructure` (see Task 2 note).

### Log-Capture Deferral (AC 6)

AC 6 offers two paths for asserting that the outage window produced a
`warn!`/`error!` log: (a) capture via `tracing_test` (or equivalent
subscriber) and assert at least one matching event, or (b) assert only
the final row counts and document the deferral.

This story takes path (b). Rationale:

- `tracing_test` requires initializing a global subscriber in a test
  binary that already co-exists with other integration tests whose log
  expectations are not controlled here; swapping subscribers per test is
  non-trivial and would affect `worker_integration_test` noise.
- The row-count assertions (20 `completed`, 0 `running`) are the true
  end-to-end contract. If reconnection were silently broken, the counter
  would not reach 20 and the 45 s timeout would fire.
- The classifier branch is independently covered by
  `worker_saturation_classifier_invoked_on_claim_error` in `worker.rs`
  and the `is_pool_timeout_*` unit tests in `db.rs`.

Follow-up tracked in `deferred-work.md` (Story 2.3 review entry):
add `tracing_test` log capture once the integration-test subscriber
story lands (Epic 3 / Story 3.1 structured logging).

### Test Strategy

**Unit tests:**

- `default_idle_timeout_is_finite`, `default_max_lifetime_is_finite`, `default_min_connections_is_zero` in `db.rs`.
- `task_error_storage_preserves_sqlx_source` in `infrastructure/src/error.rs` (or sibling).
- `is_pool_saturation_positive` / `is_pool_saturation_negative` wherever the helper lives.
- `worker_continues_after_claim_error` in `worker.rs`.

**Integration tests (testcontainers):**

- `postgres_outage_survives_reconnection` — marquee test for this story. Isolated container. Stops/starts Postgres mid-run. 20 tasks, zero loss, `timeout(45s)` guard.

**Explicitly out-of-scope tests:**

- Process-level SIGKILL on Postgres (vs container stop/start) — belongs to Epic 5 chaos suite.
- Metric emission assertions for `pool_wait_queue_depth` — Story 3.2.
- Network partition / iptables-based chaos — Epic 5.

### Project Structure Notes

- **New files:**
  - `crates/api/tests/db_outage_integration_test.rs` — outage integration test.
  - `docs/guidelines/postgres-reconnection.md` (OR a README.md section — implementer's choice).
- **Modified files:**
  - `crates/infrastructure/src/db.rs` — new constants, extended `PgPoolOptions` chain, new unit tests.
  - `crates/infrastructure/src/error.rs` — new source-chain preservation test (code change only if the test reveals a break).
  - `crates/application/src/services/worker.rs` — `is_pool_saturation` helper, warn-branch in error arm, new unit test.
  - (Possibly) `crates/infrastructure/src/lib.rs` — re-export of `is_pool_timeout` helper if Task 2 places it in infrastructure.
  - `docs/artifacts/implementation/deferred-work.md` — new section for the 2.3 OTel-metric deferral.

No new migrations. No schema changes. No changes to `WorkerConfig` / `DatabaseConfig` struct fields (defaults are constants, not configurable fields — figment wiring is Epic 5).

### Out of Scope

- **`pool_wait_queue_depth` OTel metric** — Story 3.2.
- **Retry-with-jitter backoff on consecutive claim errors** — already deferred; Epic 2 future story or Epic 5.
- **Dedicated chaos test directory** `crates/api/tests/chaos/` — Architecture specifies it (line 915-920), but Epic 5 owns chaos-suite infrastructure. The outage test in this story lives at `crates/api/tests/db_outage_integration_test.rs` and can be **moved** into `chaos/` when Epic 5 lands without rewriting.
- **`main.rs` standalone binary wiring `create_pool()` with real config** — Epic 4 / Epic 5.
- **figment-based configurability** of the new pool constants — Epic 5 alongside other Duration fields.
- **Kubernetes-level tuning** (termination grace periods, readiness gating) — Epic 5.
- **Migration-safe reconnection** (migrations run once at `build()` — a restart during migration is undefined) — existing pre-existing deferral in Story 1A.3 review; not addressed here.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 564-597] — Story 2.3 BDD acceptance criteria.
- [Source: `docs/artifacts/planning/architecture.md` lines 76-88] — Technical constraints (SQLx, rustls-only).
- [Source: `docs/artifacts/planning/architecture.md` lines 487-491] — Embedded library accepts caller-provided `PgPool`.
- [Source: `docs/artifacts/planning/architecture.md` lines 872] — `db.rs` is the `create_pool()` / `PgPoolOptions` wiring home.
- [Source: `docs/artifacts/planning/architecture.md` lines 925-934] — Layer dependency rules (why `sqlx` stays out of `application`).
- [Source: `docs/artifacts/planning/architecture.md` lines 952-955] — Chaos test isolation boundary (no shared `TEST_DB`).
- [Source: `docs/artifacts/planning/prd.md` line 336] — `IRON_DEFER_POOL_SIZE` default 10.
- [Source: `docs/artifacts/planning/prd.md` line 374] — Connection loss handling contract.
- [Source: `docs/artifacts/planning/prd.md` line 419] — Postgres-restart chaos scenario is mandatory.
- [Source: `docs/artifacts/planning/prd.md` line 746] — FR16 statement.
- [Source: `docs/artifacts/planning/prd.md` line 818] — NFR-R6 statement.
- [Source: `docs/artifacts/implementation/2-2-graceful-shutdown-and-lease-release.md`] — Previous story; worker split run/drain, error-preservation patterns.
- [Source: `docs/artifacts/implementation/2-1-sweeper-zombie-task-recovery.md`] — Sweeper log-and-continue pattern.
- [Source: `docs/artifacts/implementation/deferred-work.md`] — Existing `TaskError::Storage` source-chain history; `Duration::ZERO` validation deferral.
- [Source: `crates/infrastructure/src/db.rs:55-85`] — Current `create_pool()` implementation.
- [Source: `crates/application/src/services/worker.rs:100-157`] — Current poll loop + error branch.
- [Source: `crates/application/src/services/sweeper.rs:75-85`] — Current sweeper error branch.

## Change Log

- 2026-04-15 — Story implemented end-to-end. Pool resilience options hardened (`test_before_acquire`, `idle_timeout`, `max_lifetime`, `min_connections=0`). Pool-saturation classifier injected into `WorkerService` (keeps `sqlx` out of the `application` crate per architecture lines 925–934). New chaos integration test `db_outage_integration_test` validates 20 tasks survive a container stop/restart. Runbook added at `docs/guidelines/postgres-reconnection.md`. Four follow-up items recorded in `deferred-work.md`.

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context)

### Debug Log References

- Story workflow tracked in session todo list (Tasks 1–8).
- Pre-existing shared-pool flakiness in `integration_test` / `worker_integration_test` verified NOT a regression by re-running `cargo test` against `main` with changes stashed — same PoolTimedOut failures reproduced. Documented under Task 8.

### Completion Notes List

- **Layer-rule-preserving saturation detection.** The story allowed either `sqlx` in `application` or a thin helper in `infrastructure`. I chose a third path that avoids both problems: `infrastructure::is_pool_timeout(&TaskError) -> bool` does the full source-chain walk, and `WorkerService` accepts an injected `SaturationClassifier` closure (`Arc<dyn Fn(&TaskError) -> bool>`). `crates/api/src/lib.rs` wires the real classifier; unit tests use the default no-op. This keeps `sqlx` entirely out of `application/Cargo.toml`.
- **Integration-test verification pool.** After `container.stop()` + `container.start()`, the engine's long-lived pool may hold half-broken connections that slow `acquire` past `DEFAULT_ACQUIRE_TIMEOUT` on a fresh checkout. The test constructs a **separate verification pool** after the outage for the final `SELECT COUNT(*)` assertions. This doubles as proof that an external operator can reconnect cleanly post-recovery. Passes reliably: 3 consecutive runs of `cargo test -p iron-defer --test db_outage_integration_test` at 13–17 s each.
- **Pre-existing test flakiness not fixed.** `integration_test` and `worker_integration_test` (both use shared `TEST_DB OnceCell`) reproduce `PoolTimedOut` during `IronDefer::build()` on main without any Story 2.3 changes. This is tracked in deferred-work entries from stories 1A.2 and 2.2. Out of scope for Story 2.3.
- **Payload privacy (FR38) verified.** Both log branches in the poll-loop error arm carry only `worker_id`, `queue`, `event`, and the `TaskError` Display string. No task/payload fields appear.

### File List

**Modified:**

- `crates/infrastructure/src/db.rs` — Added `DEFAULT_MIN_CONNECTIONS`, `DEFAULT_IDLE_TIMEOUT`, `DEFAULT_MAX_LIFETIME` constants; extended `create_pool()` builder chain; added `is_pool_timeout(&TaskError) -> bool` helper; added 7 new unit tests.
- `crates/infrastructure/src/error.rs` — Added `task_error_storage_preserves_sqlx_pool_timeout_source` unit test.
- `crates/infrastructure/src/lib.rs` — Re-exported new constants and `is_pool_timeout`.
- `crates/application/src/services/worker.rs` — Added `SaturationClassifier` type alias, `is_saturation` field, `with_saturation_classifier` builder, warn-branch in the poll-loop error arm; added 2 unit tests (`worker_continues_after_claim_error`, `worker_saturation_classifier_invoked_on_claim_error`).
- `crates/api/src/lib.rs` — Wired `iron_defer_infrastructure::is_pool_timeout` into `WorkerService::with_saturation_classifier` at `IronDefer::start()`.
- `docs/artifacts/implementation/deferred-work.md` — New "Deferred from: implementation of 2-3-postgres-auto-reconnection (2026-04-15)" section with 4 entries.
- `docs/artifacts/implementation/sprint-status.yaml` — Status transitioned `ready-for-dev` → `in-progress` → `review`.

**New:**

- `crates/api/tests/db_outage_integration_test.rs` — Chaos integration test `postgres_outage_survives_reconnection` with isolated container helper `boot_isolated_test_db`.
- `docs/guidelines/postgres-reconnection.md` — Operator runbook for reconnection behaviour.
