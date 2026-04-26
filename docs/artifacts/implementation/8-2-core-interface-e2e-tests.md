# Story 8.2: Core Interface E2E Tests

Status: done

## Story

As a developer,
I want end-to-end tests that validate the complete task lifecycle through every interface,
so that I can verify iron-defer works correctly as a whole system, not just as isolated units.

## Acceptance Criteria

### AC1: E2E Test Infrastructure

Given E2E test infrastructure,
When tests are placed in `crates/api/tests/e2e_*.rs`,
Then each test file uses testcontainers for Postgres and starts the engine in-process (embedded mode),
And CLI tests use `assert_cmd` or `escargot` to locate the built `iron-defer` binary,
And the test infrastructure pattern is documented in a `crates/api/tests/common/e2e.rs` helper module.

### AC2: Full Task Lifecycle E2E

Given a running iron-defer engine with Postgres (via testcontainers),
When a full task lifecycle E2E test executes,
Then it verifies the complete path: submit task (via library API) → worker claims → handler executes → task completes,
And the test verifies the same lifecycle via REST API: `POST /tasks` → poll `GET /tasks/{id}` until status is `completed`,
And the test verifies via CLI: `iron-defer submit` → `iron-defer tasks --status completed` shows the task,
And all three interfaces report consistent state for the same task.

### AC3: Multi-Endpoint REST Workflow

Given a running iron-defer engine,
When a multi-endpoint REST workflow E2E test executes,
Then it performs: `POST /tasks` (create) → `GET /tasks/{id}` (verify created) → `GET /tasks?queue=default&status=pending` (verify in list) → `DELETE /tasks/{id}` (cancel) → `GET /tasks/{id}` (verify cancelled),
And each step asserts the expected HTTP status code and response body,
And the cancelled task does not appear in `GET /tasks?status=pending`.

### AC4: Error-Path E2E

Given a running iron-defer engine,
When error-path E2E scenarios execute,
Then `POST /tasks` with missing `kind` field returns HTTP 422 with `INVALID_PAYLOAD` error code,
And `GET /tasks/{id}` with a non-existent UUID returns HTTP 404 with `TASK_NOT_FOUND`,
And `DELETE /tasks/{id}` on a completed task returns HTTP 409 with `TASK_IN_TERMINAL_STATE` error code.

### AC5: CLI-to-REST Consistency

Given a running iron-defer engine,
When a CLI-to-REST consistency E2E test executes,
Then a task submitted via CLI is visible via `GET /tasks?queue=test`,
And the task's `id`, `queue`, `kind`, and `status` match between CLI output and REST response,
And the test verifies field-level consistency (not just existence).

## Tasks / Subtasks

- [x] **Task 1: Add assert_cmd dependency** (AC: 1)
  - [x] 1.1: Add `assert_cmd = "2"` to workspace `[workspace.dependencies]` in root `Cargo.toml` (note: `cargo` feature does not exist in v2; `cargo_bin()` is available by default)
  - [x] 1.2: Add `assert_cmd` to `[dev-dependencies]` in `crates/api/Cargo.toml` referencing workspace

- [x] **Task 2: Create E2E test helper module** (AC: 1)
  - [x] 2.1: Create `crates/api/tests/common/e2e.rs` with shared E2E infrastructure
  - [x] 2.2: Implement `TestServer` struct: starts engine in-process on ephemeral port, provides `base_url`, handles shutdown via `CancellationToken`
  - [x] 2.3: Implement `boot_e2e_engine(queue)` → `Option<(TestServer, PgPool)>` that uses `fresh_pool_on_shared_container()` (returns `None` when Docker unavailable for skip semantics), builds engine with `skip_migrations(true)`, spawns HTTP server and workers
  - [x] 2.4: Implement `wait_for_status(client, base_url, task_id, target_status, timeout)` → poll helper
  - [x] 2.5: Re-export from `common/mod.rs` via `pub mod e2e;`

- [x] **Task 3: Full task lifecycle E2E test** (AC: 2)
  - [x] 3.1: Create `crates/api/tests/e2e_lifecycle_test.rs`
  - [x] 3.2: Test library API path: `engine.enqueue()` → `engine.start(token)` → poll until `completed`
  - [x] 3.3: Test REST API path: `POST /tasks` → poll `GET /tasks/{id}` until `completed`
  - [x] 3.4: Test CLI path: use `assert_cmd` to invoke `iron-defer submit` and `iron-defer tasks --status completed`
  - [x] 3.5: Verify all three interfaces report identical state for the same task (id, queue, kind, status match)

- [x] **Task 4: Multi-endpoint REST workflow E2E test** (AC: 3)
  - [x] 4.1: Create `crates/api/tests/e2e_rest_workflow_test.rs`
  - [x] 4.2: Test create → read → list → cancel → verify cancel flow
  - [x] 4.3: Assert HTTP status codes at each step (201, 200, 200, 200, 200)
  - [x] 4.4: Assert cancelled task excluded from `GET /tasks?status=pending`

- [x] **Task 5: Error-path E2E test** (AC: 4)
  - [x] 5.1: Add error path scenarios to `e2e_rest_workflow_test.rs`
  - [x] 5.2: Test missing `kind` → 422 + `INVALID_PAYLOAD`
  - [x] 5.3: Test non-existent UUID → 404 + `TASK_NOT_FOUND`
  - [x] 5.4: Test cancel completed task → 409 + `TASK_IN_TERMINAL_STATE`

- [x] **Task 6: CLI-to-REST consistency E2E test** (AC: 5)
  - [x] 6.1: Create `crates/api/tests/e2e_cli_consistency_test.rs`
  - [x] 6.2: Submit via `assert_cmd` with `--json` flag, parse output for task ID
  - [x] 6.3: Query same task via REST `GET /tasks/{id}`
  - [x] 6.4: Assert field-level match: `id`, `queue`, `kind`, `status`, `priority`

### Review Findings

- [x] [Review][Patch] Race condition in e2e_cli_consistency_test.rs [crates/api/tests/e2e_cli_consistency_test.rs:1]
- [x] [Review][Patch] TestServer drop hygiene [crates/api/tests/common/e2e.rs:88]


## Dev Notes

### E2E Test Infrastructure Pattern

The existing test infrastructure in `crates/api/tests/common/mod.rs` provides the foundation:
- `fresh_pool_on_shared_container()` — per-test pool on shared testcontainer (preferred for multi-test binaries)
- `unique_queue()` — UUID-scoped queue names for test isolation
- `test_db_url()` — connection URL from shared container

The `rest_api_test.rs` already demonstrates the `TestServer` pattern:
```rust
struct TestServer {
    base_url: String,
    _engine: Arc<IronDefer>,
    token: CancellationToken,
    _handle: tokio::task::JoinHandle<()>,
}
```
The E2E helper in `common/e2e.rs` should extract and generalize this pattern rather than duplicating it. Check if `rest_api_test.rs` can also be refactored to use the new shared helper.

### CLI Testing Strategy

Two approaches exist — choose based on AC requirements:

1. **Direct function calls** (current `cli_test.rs` pattern): calls `cli::submit::run()` directly — faster, no binary build required, but tests the handler logic, not the full binary.
2. **Process spawning via `assert_cmd`** (required by AC1): invokes the compiled `iron-defer` binary — slower, requires `cargo build` as prerequisite, but validates full CLI argument parsing + binary wiring.

AC1 explicitly requires `assert_cmd` or `escargot`. Use `assert_cmd` which wraps `escargot` internally. The binary name is `iron-defer` (from `[[bin]]` in `crates/api/Cargo.toml`). The `assert_cmd::Command::cargo_bin("iron-defer")` call will trigger a build if needed.

**Critical:** CLI binary tests need `DATABASE_URL` set in the subprocess environment. Pass `test_db_url()` value via `.env("DATABASE_URL", &db_url)` on the `Command`.

### HTTP Endpoint Reference

All endpoints are registered in `crates/api/src/http/router.rs`:
- `POST /tasks` — create task (JSON body: `queue`, `kind`, `payload`, `scheduledAt`, `priority`, `maxAttempts`)
- `GET /tasks` — list tasks (query params: `queue`, `status`, `limit`, `offset`)
- `GET /tasks/{id}` — get single task
- `DELETE /tasks/{id}` — cancel task
- `GET /queues` — queue statistics
- `GET /health` — liveness (always 200)
- `GET /health/ready` — readiness (checks DB)
- `GET /metrics` — Prometheus text
- `GET /openapi.json` — OpenAPI 3.1 spec

Response bodies use camelCase field names (Architecture §D6 Serialization Decision).

### Error Response Codes

From `crates/api/src/http/errors.rs`, error responses follow the pattern:
```json
{"error": {"code": "ERROR_CODE", "message": "..."}}
```
Verified error codes:
- `INVALID_PAYLOAD` — 422, missing/invalid fields on `POST /tasks`
- `TASK_NOT_FOUND` — 404, non-existent task ID
- `TASK_IN_TERMINAL_STATE` — 409, cancel on completed/failed/cancelled task
- `TASK_ALREADY_CLAIMED` — 409, cancel on a running task
- `INVALID_QUERY_PARAMETER` — 422, bad query params on `GET /tasks`
- `TASK_NOT_IN_EXPECTED_STATE` — 409, state transition conflict

### Engine Start Method

The correct method to start workers is `engine.start(token: CancellationToken)`, NOT `start_workers()`. Workers are started for the queue configured via `.queue("name")` on the builder. The `start()` method spawns the worker pool, sweeper, and observability tasks.

### CLI `--json` Flag Placement

The `--json` flag is a **global flag** that must appear BEFORE the subcommand:
```
iron-defer --json submit --queue test --kind TestKind --payload '{}'
```
NOT `iron-defer submit --json ...`. Placing it after the subcommand causes a clap parse error.

### Anti-Patterns to Avoid

- **Do NOT create a separate binary for E2E tests** — use the in-process engine via `IronDefer::builder()` for library/REST tests, `assert_cmd` only for CLI tests
- **Do NOT use `test_pool()`** — always use `fresh_pool_on_shared_container()` for E2E tests to avoid runtime stranding
- **Do NOT hardcode ports** — use `TcpListener::bind("127.0.0.1:0")` for ephemeral ports
- **Do NOT sleep for fixed durations** — use the poll helper with timeout for status transitions
- **Do NOT skip migrations in CLI binary tests** — the binary runs its own migration on startup; the pool must be fresh
- **Do NOT duplicate existing test logic** — `rest_api_test.rs` already has `cancel_pending_task_returns_200`, `get_nonexistent_task_returns_404`, `post_task_missing_kind_returns_422`, `cancel_completed_task_returns_409`. AC3 and AC4 E2E tests should focus on the WORKFLOW (multi-step sequenced operations) rather than re-testing individual error codes already covered by unit-level REST tests

### Testing Strategy

All E2E tests go in `crates/api/tests/e2e_*.rs`. Run with:
```bash
cargo test --package iron-defer -j1 -- e2e_
```
Use `-j1` to avoid port/container contention between E2E tests that spawn HTTP servers.

### Previous Story Intelligence

**From Story 8.1 (done):**
- Architecture document fully reconciled — all file paths, function names, and patterns are current
- 25+ source file comments updated to section-name refs (no more stale line-number refs)
- `ListTasksFilter` is the correct type name (not `TaskFilter`)
- `cargo check --workspace` passes cleanly

**From Epic 7 retrospective:**
- Zero-debugging implementations are reproducible when story context is thorough
- Pre/postcondition discipline: verify outcomes, not just command execution

### Project Structure Notes

- E2E test files: `crates/api/tests/e2e_*.rs` (new, naming convention distinguishes from existing integration tests)
- E2E helper: `crates/api/tests/common/e2e.rs` (new module in existing common/ directory)
- Dependencies: `assert_cmd` added to workspace and api crate dev-deps

### References

- [Source: docs/artifacts/planning/epics.md, Lines 758-796 — Story 8.2 definition, CR47-CR50]
- [Source: crates/api/tests/common/mod.rs — existing test helpers: fresh_pool_on_shared_container, unique_queue, test_db_url]
- [Source: crates/api/tests/rest_api_test.rs — TestServer pattern for in-process HTTP testing]
- [Source: crates/api/tests/cli_test.rs — CLI direct function call testing pattern]
- [Source: crates/api/tests/chaos_db_outage_test.rs — chaos test isolation pattern]
- [Source: crates/api/src/http/router.rs — all HTTP endpoint registrations]
- [Source: crates/api/src/http/handlers/tasks.rs — CreateTaskRequest, TaskResponse, error handling]
- [Source: crates/api/src/http/errors.rs — error response format and codes]
- [Source: crates/api/src/cli/mod.rs — CLI subcommand dispatch]
- [Source: crates/api/src/cli/submit.rs — Submit handler: queue, kind, payload, scheduled-at, priority, max-attempts flags]
- [Source: crates/api/Cargo.toml — current dev-dependencies (testcontainers, reqwest)]
- [Source: docs/artifacts/implementation/8-1-architecture-reconciliation-and-engineering-standards.md — previous story learnings]
- [Source: docs/artifacts/planning/architecture.md §D6 — camelCase serialization convention]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- assert_cmd v2 does not have a `cargo` feature; `cargo_bin()` is available by default — corrected from story spec
- `QueueName` does not implement `AsRef<str>`; use `.as_str()` instead
- Race condition fix: `e2e_cli_list_matches_rest` required submitting to a queue without workers to prevent status change between REST and CLI reads

### Completion Notes List

- 8 E2E tests across 3 test files, all passing
- E2E helper module (`common/e2e.rs`) provides `TestServer`, `boot_e2e_engine(queue)`, and `wait_for_status()` poll helper
- Full lifecycle verified through all 3 interfaces: library API, REST API, CLI binary (via `assert_cmd`)
- All three interfaces report consistent field values for the same task
- Multi-step REST workflow validates create→read→list→cancel→verify→exclude-from-pending
- Error-path workflow validates 422/404/409 error codes in sequenced scenario
- CLI-to-REST field-level consistency verified for id, queue, kind, status, priority
- Full regression suite passes (zero failures)

### Change Log

- 2026-04-24: Implemented all 6 tasks for Story 8.2 — E2E test infrastructure + 8 test functions

### File List

- Cargo.toml (modified — added `assert_cmd` workspace dependency)
- crates/api/Cargo.toml (modified — added `assert_cmd` dev-dependency)
- crates/api/tests/common/mod.rs (modified — added `pub mod e2e;`)
- crates/api/tests/common/e2e.rs (new — E2E test helper: TestServer, boot_e2e_engine, wait_for_status)
- crates/api/tests/e2e_lifecycle_test.rs (new — 4 tests: library API, REST API, CLI, cross-interface consistency)
- crates/api/tests/e2e_rest_workflow_test.rs (new — 2 tests: multi-step REST workflow, error-path workflow)
- crates/api/tests/e2e_cli_consistency_test.rs (new — 2 tests: CLI-to-REST field consistency, CLI-list-matches-REST)
