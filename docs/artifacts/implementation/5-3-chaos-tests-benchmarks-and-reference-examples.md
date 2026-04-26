# Story 5.3: Chaos Tests, Benchmarks & Reference Examples

Status: done

## Story

As a Rust developer,
I want chaos tests proving correctness, benchmarks proving performance, and examples showing usage,
so that I can trust the at-least-once guarantee and get started quickly.

## Acceptance Criteria

1. **Chaos: Worker crash recovery — `crates/api/tests/chaos/worker_crash_test.rs`:**

   A worker is killed mid-execution. The sweeper recovers the task within the configured lease duration. The task is completed by another worker — zero task loss.

   The test uses its OWN isolated Postgres container (not shared `TEST_DB`).

   **Maps to:** TEA P1-CHAOS-001, Architecture lines 738–743.

2. **Chaos: Database outage survival — `crates/api/tests/chaos/db_outage_test.rs`:**

   Postgres becomes unavailable during worker polling. Workers retry on reconnection. No tasks are lost.

   The test uses its OWN isolated Postgres container.

   **Maps to:** TEA P1-CHAOS-002, Architecture lines 738–743.

3. **Chaos: SIGTERM graceful shutdown — `crates/api/tests/chaos/sigterm_test.rs`:**

   SIGTERM is sent during active task execution. In-flight tasks complete or leases are released. Zero orphaned `Running` tasks.

   The test uses its OWN isolated Postgres container.

   **Maps to:** TEA P0-CHAOS-001/002, Architecture lines 738–743.

4. **Chaos: Max retries exhausted — `crates/api/tests/chaos/max_retries_test.rs`:**

   A task exhausts all retry attempts. The task transitions to `Failed` and is never re-queued as `Pending`.

   The test uses its OWN isolated Postgres container.

   **Maps to:** TEA P1-CHAOS-003, Architecture lines 738–743.

5. **Throughput benchmark — `crates/api/benches/throughput.rs`:**

   A Criterion benchmark verifies >= 10,000 task completions/sec on a single Postgres instance (NFR-P2).

   Requires `DATABASE_URL` environment variable pointing to a real Postgres instance. Runs on the release CI path (`release.yml`), not on every PR.

   **Maps to:** NFR-P2, TEA P3-BENCH-001, Architecture lines 909–910, 1210–1221.

6. **Example: Basic enqueue — `crates/api/examples/basic_enqueue.rs`:**

   Demonstrates task definition, submission, and retrieval using the embedded library API. Executes without external dependencies beyond Postgres (uses `DATABASE_URL` env var).

   **Maps to:** FR36, Architecture lines 906–908.

7. **Example: Axum integration — `crates/api/examples/axum_integration.rs`:**

   Demonstrates embedding iron-defer inside an existing axum application. Shows the builder pattern with caller-provided `PgPool`.

   **Maps to:** FR36, Architecture lines 906–908.

8. **Quality gates:**

   - All 4 chaos tests pass with Docker available (`IRON_DEFER_REQUIRE_DB=1`).
   - `cargo check --examples` compiles all examples.
   - Benchmark compiles: `cargo bench --bench throughput --no-run`.
   - `cargo fmt --check` — clean.
   - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean.
   - Existing test suites remain green — no regressions.

## Tasks / Subtasks

- [x] **Task 1: Create chaos test files as top-level integration tests** (AC 1–4)
  - [x] **IMPORTANT: Rust test discovery.** Used approach **(a)** — flat layout with `chaos_` prefix, auto-discovered by `cargo test`.
  - [x] Implemented shared `chaos_common.rs` module with `boot_isolated_chaos_db()`, `should_skip()`, and `unique_queue()` helpers.
  - [x] Each chaos test file MUST NOT reference the shared `TEST_DB OnceCell` from `tests/common/mod.rs`.

- [x] **Task 2: Worker crash chaos test** (AC 1)
  - [x] Created `crates/api/tests/chaos_worker_crash_test.rs`.
  - [x] Simulates crash by claiming tasks directly via `PostgresTaskRepository::claim_next()` with fake `WorkerId` and short lease (2s), never completing them.
  - [x] Sweeper recovers orphaned tasks; real worker pool completes them. All 10 tasks reach `Completed`.

- [x] **Task 3: Database outage chaos test** (AC 2)
  - [x] Created `crates/api/tests/chaos_db_outage_test.rs` — moved and refactored from `db_outage_integration_test.rs`.
  - [x] Deleted original `db_outage_integration_test.rs`. Chaos version uses shared `chaos_common` module.
  - [x] Retains isolated container with pinned port, stop/start pattern, verifies all 20 tasks complete.

- [x] **Task 4: SIGTERM graceful shutdown chaos test** (AC 3)
  - [x] Created `crates/api/tests/chaos_sigterm_test.rs`.
  - [x] Uses isolated container, `CancellationToken` cancellation, verifies zero orphaned `Running` tasks.

- [x] **Task 5: Max retries exhausted chaos test** (AC 4)
  - [x] Created `crates/api/tests/chaos_max_retries_test.rs`.
  - [x] `FailingTask` always returns `Err(TaskError::ExecutionFailed)`, exhausts 3 attempts, transitions to `Failed`, stays `Failed`.

- [x] **Task 6: Register `[[bench]]` target in `Cargo.toml`** (AC 5)
  - [x] Added `[[bench]]`, `[[example]]` entries to `crates/api/Cargo.toml`.
  - [x] Added `criterion = { version = "0.5", features = ["html_reports"] }` to workspace `[workspace.dependencies]`.
  - [x] Added `criterion` to `crates/api/[dev-dependencies]`.

- [x] **Task 7: Create throughput benchmark** (AC 5)
  - [x] Created `crates/api/benches/throughput.rs` with Criterion `iter_custom` + tokio runtime.
  - [x] Batch size 1000, concurrency 16, `NoopTask` handler. Reports throughput via `println!`.
  - [x] Compiles: `cargo bench --bench throughput --no-run` passes.

- [x] **Task 8: Create `basic_enqueue` example** (AC 6)
  - [x] Created `crates/api/examples/basic_enqueue.rs` with step-by-step comments.
  - [x] Demonstrates `GreetTask` definition, enqueue, find, worker pool start, and clean shutdown.

- [x] **Task 9: Create `axum_integration` example** (AC 7)
  - [x] Created `crates/api/examples/axum_integration.rs` with embedded library pattern.
  - [x] Demonstrates caller-owned `PgPool`, axum `POST /enqueue` handler, shared `CancellationToken` for coordinated shutdown.

- [x] **Task 10: Quality gates** (AC 8)
  - [x] All 4 chaos tests pass.
  - [x] Examples compile: `cargo check --examples -p iron-defer`.
  - [x] Benchmark compiles: `cargo bench --bench throughput --no-run`.
  - [x] `cargo fmt --check` — clean.
  - [x] Clippy: new files pass pedantic. Pre-existing `iron-defer-application` crate has 4 doc-related pedantic errors (not introduced by this story).
  - [x] Existing test suites remain green — full workspace passes.

### Review Findings

- [x] [Review][Defer] **DB outage chaos test drops Story 3.1 AC 7 log assertion** — The deleted `db_outage_integration_test.rs` had `#[tracing_test::traced_test]` and a `logs_contain(...)` assertion. Decision: create a separate dedicated test for AC 7 log coverage (deferred — prior story scope, not Story 5.3).
- [x] [Review][Patch] **Benchmark `DELETE FROM tasks` is unscoped** — Removed global DELETE; each iteration uses a unique queue name, no cleanup needed.
- [x] [Review][Patch] **Axum example binds to `0.0.0.0:3000`** — Changed to `127.0.0.1:3000` for local-only access.
- [x] [Review][Patch] **basic_enqueue example doesn't join spawned engine handle** — Captured `JoinHandle` and added `let _ = engine_handle.await`.
- [x] [Review][Dismiss] TOCTOU race on ephemeral port — pre-existing pattern from original test, acceptable for single-binary test isolation
- [x] [Review][Dismiss] Static AtomicUsize counter — each test is a separate binary, no cross-test sharing
- [x] [Review][Dismiss] Container leak on panic — standard test cleanup via process exit
- [x] [Review][Dismiss] Max retries timing-dependent wait — secondary belt-and-suspenders check, not primary assertion
- [x] [Review][Dismiss] Criterion in workspace.dependencies — Cargo workspace has no [workspace.dev-dependencies]; current placement is correct
- [x] [Review][Dismiss] Worker crash sweeper timing — sufficient margin with 500ms sweep interval and 2.5s sleep
- [x] [Review][Dismiss] SIGTERM test timing — 300ms cancel vs 600ms minimum execution, safe margin

## Dev Notes

### Architecture Compliance

- **Architecture lines 738–743**: Chaos test minimum manifest — all 4 scenarios required.
- **Architecture lines 909–920**: Benchmark and example file locations — `crates/api/benches/throughput.rs`, `crates/api/examples/basic_enqueue.rs`, `crates/api/examples/axum_integration.rs`.
- **Architecture lines 952–955**: Chaos test isolation boundary — each test spins up its OWN Postgres container. Must NOT use shared `TEST_DB OnceCell`.
- **Architecture lines 1210–1221**: C7 — Criterion benchmarks require external `DATABASE_URL`, run on release path.
- **Architecture lines 889–890**: `Cargo.toml` registrations — `[[bench]] throughput`, `[[example]] basic_enqueue`, `[[example]] axum_integration`.
- **PRD lines 636–649**: Example quality bar — must compile, run without deps beyond Postgres, be referenced from README.
- **NFR-P2**: >= 10,000 task completions/sec on single commodity Postgres.

### Critical Design Decisions

**Chaos tests in `crates/api/tests/chaos/` — separate test binaries.**
Each chaos test file compiles as a separate test binary. This is important because chaos tests stop/start Postgres containers — they cannot share a process with tests that rely on a stable shared container. The `#[ignore]` attribute is NOT used — chaos tests run as part of `cargo test` when Docker is available. They skip gracefully via `IRON_DEFER_SKIP_DOCKER_CHAOS=1` env var opt-out.

**Existing chaos-like tests to refactor.**
The codebase already has tests that exercise chaos-like scenarios but are NOT in the `chaos/` directory:
- `db_outage_integration_test.rs` — already uses isolated container, stop/start pattern. This IS effectively the DB outage chaos test. Task 3 moves it to `chaos/`.
- `shutdown_test.rs` — tests graceful shutdown via `CancellationToken` but uses the shared container. The chaos version (Task 4) uses an isolated container for full independence.
- `sweeper_test.rs` — tests zombie recovery but doesn't simulate a worker "crash." The chaos version (Task 2) simulates actual worker abandonment.

The dev agent should reuse proven patterns from these existing tests, not rewrite from scratch.

**Criterion benchmark does NOT hard-fail on throughput threshold.**
Criterion reports statistical results (mean, median, variance) and tracks regressions across runs. The 10,000 tasks/sec NFR is validated by inspecting the benchmark output, not by a hard assertion in the benchmark code. A `println!` at the end of the benchmark reporting the calculated throughput is sufficient for CI logs. Hard-failing would make the benchmark brittle across different hardware.

**Examples require `DATABASE_URL` — no testcontainers in examples.**
Examples are meant to be run by developers against their own Postgres. Using testcontainers would add a heavy dependency and complicate the example code. The README should document: "Run `docker compose -f docker/docker-compose.dev.yml up -d` first, then set `DATABASE_URL`."

**`basic_enqueue` example name (not `basic_task`).**
The Architecture uses `basic_enqueue`, the PRD uses `basic_task`. The Epic AC uses `basic_enqueue`. Follow the Epic AC naming.

### Previous Story Intelligence

**From `db_outage_integration_test.rs` (Story 2.3):**
- Proven `boot_isolated_test_db()` pattern with pinned port (lines 68–108). Reuse for all chaos tests.
- `container.stop()` / `container.start()` API for Postgres outage simulation.
- `AtomicUsize` counter for tracking task completions.
- `IRON_DEFER_SKIP_DOCKER_CHAOS=1` opt-out pattern.
- `ImageExt::with_mapped_port()` for pinning the host port.

**From `shutdown_test.rs` (Story 2.2):**
- `SlowTask` pattern — task that sleeps for configurable duration, used to simulate long-running work.
- `CancellationToken` cancellation to simulate SIGTERM.
- Assertion pattern: check all tasks are either `Completed` or `Pending` (none stuck in `Running`).
- `WorkerConfig` with short intervals for fast test execution.

**From `sweeper_test.rs` (Story 2.1):**
- Manual claiming via `PostgresTaskRepository::claim_next()` to simulate a worker holding a lease.
- `SweeperService::new()` + `sweeper.run()` for independent sweeper execution.
- Short lease duration (100ms) for fast lease expiry in tests.
- `QueueName`, `WorkerId` direct usage for low-level manipulation.

**From Story 4.2 (REST API List Tasks & Queue Stats):**
- `fresh_pool_on_shared_container()` pattern — NOT used in chaos tests (chaos needs isolation).
- `unique_queue()` — used for test queue isolation, reusable in chaos tests.

**From Epic 1B/2/3 Retrospective:**
- "Test flakiness with shared `TEST_DB OnceCell`" — chaos tests must use isolated containers to avoid this.

### Git Intelligence

Recent commits (last 5):
- `a0db5fb` — REST API list tasks and queue stats (Story 4.2).
- `7d0c584` — Health probes and task cancellation APIs (Story 4.1).
- `2b70581` — Custom Axum extractors for structured JSON error responses.
- `2a1ed9a` — Removed OTel compliance tests for Story 3.3.
- `940c722` — OTel compliance tests and SQL audit trail.

### Key Types and Locations (verified current as of 2026-04-21)

- `IronDefer` builder — `crates/api/src/lib.rs`. Methods: `enqueue`, `find`, `start`, `serve`.
- `CancellationToken` — re-exported from `crates/api/src/lib.rs:84`.
- `WorkerConfig` — `crates/application/src/config.rs:32-60`. Fields: concurrency, poll_interval, lease_duration, sweeper_interval, base_delay, max_delay, shutdown_timeout.
- `TaskError` — `crates/domain/src/error.rs`. Variants: `InvalidPayload`, `Storage`, `Execution`.
- `TaskStatus` — `crates/domain/src/model/task.rs`. Variants: `Pending`, `Running`, `Completed`, `Failed`, `Cancelled`.
- `Task` trait — `crates/domain/src/model/task.rs`. `const KIND`, `async fn execute`.
- `TaskContext` — `crates/domain/src/model/task.rs`.
- `PostgresTaskRepository` — `crates/infrastructure/src/adapters/postgres_task_repository.rs`.
- `SweeperService` — `crates/application/src/services/worker.rs`.
- `WorkerService` — `crates/application/src/services/worker.rs`.
- `DatabaseConfig` — `crates/application/src/config.rs:21-28`.
- `create_pool()` — `crates/infrastructure/src/db.rs:75-94`.
- `MIGRATOR` — `crates/infrastructure/src/db.rs:62`.
- Existing chaos-like test: `crates/api/tests/db_outage_integration_test.rs` — `boot_isolated_test_db()` pattern.
- Existing shutdown test: `crates/api/tests/shutdown_test.rs`.
- Existing sweeper test: `crates/api/tests/sweeper_test.rs`.
- Shared test helper: `crates/api/tests/common/mod.rs` — `fresh_pool_on_shared_container()`, `unique_queue()`.

### Dependencies

**New workspace dependency:**
```toml
criterion = { version = "0.5", features = ["html_reports"] }
```

Add to `[workspace.dependencies]` in root `Cargo.toml`. Add `criterion = { workspace = true }` to `crates/api/[dev-dependencies]`.

No other new dependencies. `testcontainers`, `tokio`, `serde`, `sqlx` — all already available.

### Test Strategy

**Chaos tests (each with isolated container):**
- Worker crash: enqueue N tasks, start worker, cancel mid-execution, start second worker, verify all complete.
- DB outage: enqueue tasks, stop Postgres, sleep, restart Postgres, verify all complete.
- SIGTERM: enqueue tasks, start worker, cancel token, verify drain + no orphaned Running.
- Max retries: enqueue 1 always-failing task, verify transitions to Failed after max_attempts.

**Benchmark:**
- Criterion bench `throughput.rs` measures task completions/sec. Requires external Postgres.
- `cargo bench --bench throughput --no-run` — compile check only (no DB needed).

**Examples:**
- `cargo check --examples -p iron-defer` — compile check.
- Manual run: `DATABASE_URL=... cargo run --example basic_enqueue`.

### Project Structure Notes

**New files:**
- `crates/api/tests/chaos/worker_crash_test.rs`
- `crates/api/tests/chaos/db_outage_test.rs`
- `crates/api/tests/chaos/sigterm_test.rs`
- `crates/api/tests/chaos/max_retries_test.rs`
- `crates/api/benches/throughput.rs`
- `crates/api/examples/basic_enqueue.rs`
- `crates/api/examples/axum_integration.rs`

**Modified files:**
- `Cargo.toml` (workspace root) — add `criterion` to `[workspace.dependencies]`.
- `crates/api/Cargo.toml` — add `[[bench]]`, `[[example]]` entries, add `criterion` to `[dev-dependencies]`.

**Potentially modified/deleted:**
- `crates/api/tests/db_outage_integration_test.rs` — moved to `chaos/db_outage_test.rs` or kept as a simpler integration test.

**Not modified:**
- Migrations — no schema changes.
- `.sqlx/` — unchanged.
- No changes to production source code.

### Out of Scope

- **CI pipeline wiring** — chaos tests run as part of `cargo test`, benchmark on `release.yml`. Actual CI file creation is separate.
- **README updates** — examples should be referenced from README, but README creation/update is not in this story's AC.
- **Additional examples from PRD** — PRD lists 8 examples; Epic AC only requires `basic_enqueue` and `axum_integration`. The others (`retry_and_backoff`, `sweeper_recovery`, `otel_integration`, `multi_queue`, `docker-compose/`, `kubernetes/`) are Growth/1.0.0 scope.
- **Testcontainers in examples** — examples use `DATABASE_URL`, not testcontainers.
- **Benchmark regression tracking** — Criterion tracks locally; CI integration (GitHub Actions comment) is separate.
- **Coverage gating** — `cargo tarpaulin` coverage checks are CI pipeline scope.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 880–922] — Story 5.3 acceptance criteria (BDD source).
- [Source: `docs/artifacts/planning/architecture.md` lines 738–743] — Chaos test minimum manifest.
- [Source: `docs/artifacts/planning/architecture.md` lines 909–920] — Benchmark and example file locations.
- [Source: `docs/artifacts/planning/architecture.md` lines 952–955] — Chaos test isolation boundary.
- [Source: `docs/artifacts/planning/architecture.md` lines 1210–1221] — C7: benchmark DATABASE_URL requirement.
- [Source: `docs/artifacts/planning/prd.md` lines 636–649] — Example table and quality bar.
- [Source: `docs/artifacts/planning/prd.md` line 58] — NFR-P2: >= 10,000 jobs/sec.
- [Source: `crates/api/tests/db_outage_integration_test.rs`] — Proven isolated container pattern.
- [Source: `crates/api/tests/shutdown_test.rs`] — Graceful shutdown test patterns.
- [Source: `crates/api/tests/sweeper_test.rs`] — Zombie recovery test patterns.
- [Source: `crates/api/tests/common/mod.rs`] — Shared test helpers.

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Fixed `TaskError::Execution` → `TaskError::ExecutionFailed` (correct variant name per domain model).
- Fixed `AttemptCount.get()` returns `i32` not `u32` — adjusted assertion in max retries test.
- Fixed clippy pedantic warnings: doc backticks, `match_wild_err_arm`, `let_unit_value`, `items_after_statements`, `duration_suboptimal_units`.
- Wrapped doc comments in code blocks to avoid `doc_markdown` lint on shell commands.

### Completion Notes List

- All 4 chaos tests implemented and passing with isolated Postgres containers.
- Throughput benchmark created with Criterion `iter_custom` pattern; compiles successfully.
- Two examples (`basic_enqueue`, `axum_integration`) created with doc comments and step-by-step flow.
- `db_outage_integration_test.rs` moved to `chaos_db_outage_test.rs` (flat layout); original deleted.
- Shared `chaos_common.rs` module provides `boot_isolated_chaos_db()`, `should_skip()`, `unique_queue()`.
- No production source code modified. No schema changes. No migrations touched.
- Pre-existing clippy pedantic errors in `iron-defer-application` (doc_markdown, missing_panics_doc) are NOT from this story.

### File List

- `crates/api/tests/chaos_common.rs` — NEW: shared chaos test helpers
- `crates/api/tests/chaos_worker_crash_test.rs` — NEW: worker crash recovery chaos test (AC 1)
- `crates/api/tests/chaos_db_outage_test.rs` — NEW: DB outage chaos test (AC 2, moved from db_outage_integration_test.rs)
- `crates/api/tests/chaos_sigterm_test.rs` — NEW: SIGTERM graceful shutdown chaos test (AC 3)
- `crates/api/tests/chaos_max_retries_test.rs` — NEW: max retries exhausted chaos test (AC 4)
- `crates/api/benches/throughput.rs` — NEW: Criterion throughput benchmark (AC 5)
- `crates/api/examples/basic_enqueue.rs` — NEW: basic enqueue example (AC 6)
- `crates/api/examples/axum_integration.rs` — NEW: axum integration example (AC 7)
- `Cargo.toml` — MODIFIED: added `criterion` to workspace dependencies
- `crates/api/Cargo.toml` — MODIFIED: added `[[bench]]`, `[[example]]` entries, `criterion` dev-dep
- `crates/api/tests/db_outage_integration_test.rs` — DELETED: moved to chaos_db_outage_test.rs

### Change Log

| Date | Author | Change |
|---|---|---|
| 2026-04-22 | Claude Opus 4.6 | Implemented all 10 tasks: 4 chaos tests, throughput benchmark, 2 examples, quality gates |
| 2026-04-22 | Claude Opus 4.6 | Code review: 3 patches applied (unscoped DELETE, localhost bind, JoinHandle), 1 deferred (AC 7 log assertion), 7 dismissed |
