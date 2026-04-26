# Story 12.1: HITL Suspend/Resume

Status: done

## Story

As a developer building workflows requiring human approval,
I want to suspend a task (yielding its worker slot) and resume it via an external signal,
so that human review doesn't waste worker concurrency.

## Acceptance Criteria

1. **Given** a task handler calling `ctx.suspend()`
   **When** suspend executes
   **Then** the task transitions to `Suspended` status
   **And** checkpoint data is persisted (G6 dependency)
   **And** the worker slot (including DB connection and SKIP LOCKED lock) is released
   **And** `suspended_at` timestamp is recorded

2. **Given** a suspended task
   **When** `POST /tasks/{id}/signal` is called with a JSON body
   **Then** the task transitions back to `Pending` with the signal payload stored
   **And** on the next execution, `ctx.signal_payload()` returns the signal data

3. **Given** two concurrent signals to the same suspended task
   **When** both arrive simultaneously
   **Then** exactly one succeeds (200), the other receives 409 TASK_NOT_IN_EXPECTED_STATE

4. **Given** a task suspended longer than `suspend_timeout` (default 24h)
   **When** the Sweeper's suspend watchdog tick fires
   **Then** the task is auto-failed with `last_error = "suspend timeout exceeded"`

5. **Given** suspended tasks
   **When** worker concurrency is measured
   **Then** suspended tasks do NOT count against the queue's concurrency limit

## Functional Requirements Coverage

- **FR60:** Task handler suspends execution via `ctx.suspend()`, transitions to `Suspended`, yields worker slot with checkpoint
- **FR61:** External caller resumes via `POST /tasks/{id}/signal` with payload; concurrent signals → exactly one 200, rest 409
- **FR62:** Suspended tasks excluded from concurrency limit and sweeper zombie recovery
- **FR63:** Suspend watchdog on sweeper tick auto-fails tasks past `suspend_timeout`

## Tasks / Subtasks

- [x] Task 1: Domain — SuspendSignal type and TaskError variant (AC: 1)
  - [x] 1.1 Create `SuspendSignal` struct in `crates/domain/src/model/task.rs` — carries optional checkpoint data to persist before suspend
  - [x] 1.2 Add `TaskError::SuspendRequested` variant (or reuse existing error model) — signals the worker loop that the handler wants to suspend rather than complete/fail. This is NOT an error — it's a control-flow signal. Consider a separate `TaskOutcome` enum if cleaner: `enum TaskOutcome { Completed, Suspended { checkpoint: Option<Value> } }`
  - [x] 1.3 Export from domain crate

- [x] Task 2: TaskContext.suspend() method (AC: 1)
  - [x] 2.1 Add `pub async fn suspend(&self) -> Result<(), TaskError>` to `TaskContext` in `crates/domain/src/model/task.rs`
  - [x] 2.2 Implementation: (a) call `self.checkpoint(data)` if checkpoint data is provided (G6 dependency), (b) return a special error variant or suspend signal that the worker dispatch loop recognizes
  - [x] 2.3 **Critical design decision:** `ctx.suspend()` cannot directly UPDATE the database or release the SKIP LOCKED lock — it's in the domain layer with no DB access. Instead, it returns a sentinel value (via `TaskError` variant or by modifying execution flow) that the worker dispatch function (`dispatch_task()` in worker.rs) intercepts. The actual DB UPDATE and lock release happen in `dispatch_task()`.
  - [x] 2.4 Add `pub fn signal_payload(&self) -> Option<&serde_json::Value>` accessor to `TaskContext` — returns signal data from previous suspend/signal cycle
  - [x] 2.5 Add `pub(crate) signal_payload: Option<serde_json::Value>` field to `TaskContext` struct
  - [x] 2.6 Update `TaskContext::new()` and `with_checkpoint()` builder methods to accept signal_payload parameter (or add a new `.with_signal_payload()` builder method)

- [x] Task 3: TaskRepository.suspend() port method (AC: 1)
  - [x] 3.1 Add `async fn suspend(&self, task_id: TaskId) -> Result<TaskRecord, TaskError>` to `TaskRepository` trait in `crates/application/src/ports/task_repository.rs`
  - [x] 3.2 Update `#[cfg_attr(test, mockall::automock)]` — MockTaskRepository gains suspend()
  - [x] 3.3 The method transitions `Running → Suspended`, sets `suspended_at = now()`, clears `claimed_by` and `claimed_until` (releases the logical claim)

- [x] Task 4: PostgresTaskRepository.suspend() implementation (AC: 1)
  - [x] 4.1 Implement `suspend()` in `crates/infrastructure/src/adapters/postgres_task_repository.rs`
  - [x] 4.2 SQL: `UPDATE tasks SET status = 'suspended', suspended_at = now(), claimed_by = NULL, claimed_until = NULL, updated_at = now() WHERE id = $1 AND status = 'running' RETURNING *`
  - [x] 4.3 The WHERE `status = 'running'` guard ensures atomicity — if the task is not running, the update fails gracefully (return error)
  - [x] 4.4 If audit_log is enabled, wrap in transaction and insert audit row (`running → suspended`)
  - [x] 4.5 **SKIP LOCKED release:** Clearing `claimed_by` and `claimed_until` is sufficient — the SKIP LOCKED advisory lock is on the row itself and is released when the database transaction commits. Since the worker's claim transaction already committed at claim time, there is no outstanding lock to release. The worker simply stops processing the task.

- [x] Task 5: Worker dispatch — handle suspend signal (AC: 1, 5)
  - [x] 5.1 In `dispatch_task()` (`crates/application/src/services/worker.rs:656`), add a new match arm for the suspend signal in the `handler_result` match
  - [x] 5.2 When handler returns suspend signal: (a) call `ctx.repo.suspend(task.id())`, (b) emit OTel event `task.state_transition` (running → suspended), (c) emit `task_suspended` lifecycle log, (d) do NOT call `repo.complete()` or `repo.fail()`, (e) return — the worker slot is released by function exit
  - [x] 5.3 **Critical:** The worker slot (semaphore permit, JoinSet entry) is released when `dispatch_task()` returns. No explicit semaphore release needed — the function return handles it. The DB connection is released when the function's `&DispatchContext` borrows end.
  - [x] 5.4 If metrics are available, do NOT increment `task_failures_total` (suspend is not a failure). Consider a new `iron_defer_tasks_suspended_total` counter with `queue` label.
  - [x] 5.5 Add `task_suspended` to emit function (similar pattern to `emit_task_completed`)

- [x] Task 6: POST /tasks/{id}/signal REST endpoint (AC: 2, 3)
  - [x] 6.1 Create `SignalTaskRequest` struct: `pub struct SignalTaskRequest { pub payload: Option<serde_json::Value> }`
  - [x] 6.2 Add `signal_task()` handler in `crates/api/src/http/handlers/tasks.rs` — extracts `Path(id)` and `Json(request)`
  - [x] 6.3 Implementation: calls `repo.signal(task_id, payload)` which atomically transitions `Suspended → Pending` and stores signal_payload
  - [x] 6.4 Success: return 200 with updated `TaskResponse`
  - [x] 6.5 Task not found: return 404
  - [x] 6.6 Task not in Suspended status: return 409 with error code `TASK_NOT_IN_EXPECTED_STATE`
  - [x] 6.7 Register route in `crates/api/src/http/router.rs`: `.route("/tasks/{id}/signal", post(tasks::signal_task))`
  - [x] 6.8 Add `#[utoipa::path(...)]` for OpenAPI documentation

- [x] Task 7: TaskRepository.signal() port method (AC: 2, 3)
  - [x] 7.1 Add `async fn signal(&self, task_id: TaskId, payload: Option<serde_json::Value>) -> Result<TaskRecord, TaskError>` to `TaskRepository` trait
  - [x] 7.2 Update MockTaskRepository

- [x] Task 8: PostgresTaskRepository.signal() implementation (AC: 2, 3)
  - [x] 8.1 SQL: `UPDATE tasks SET status = 'pending', signal_payload = $1, suspended_at = NULL, scheduled_at = now(), updated_at = now() WHERE id = $2 AND status = 'suspended' RETURNING *`
  - [x] 8.2 WHERE `status = 'suspended'` ensures concurrent signals: only one UPDATE succeeds (atomic). The second signal finds `status != 'suspended'` and returns zero rows → 409 response
  - [x] 8.3 If audit_log is enabled, wrap in transaction and insert audit row (`suspended → pending`)
  - [x] 8.4 Return error (not TaskRecord) when zero rows are affected — map to appropriate 404/409

- [x] Task 9: Sweeper suspend watchdog (AC: 4)
  - [x] 9.1 Add `suspend_timeout: Duration` field to `SweeperService` in `crates/application/src/services/sweeper.rs`
  - [x] 9.2 Add watchdog clause to sweeper `run()` method — called on each tick after zombie recovery and idempotency key cleanup
  - [x] 9.3 Add `async fn expire_suspended_tasks(&self, suspend_timeout: Duration) -> Result<Vec<(TaskId, QueueName)>, TaskError>` to `TaskRepository` trait
  - [x] 9.4 SQL: `UPDATE tasks SET status = 'failed', last_error = 'suspend timeout exceeded', updated_at = now() WHERE status = 'suspended' AND suspended_at < now() - $1 RETURNING id, queue`
  - [x] 9.5 If audit_log is enabled, insert audit rows (`suspended → failed`)
  - [x] 9.6 Emit `iron_defer_suspend_timeout_total` counter metric with `queue` label
  - [x] 9.7 Log: `warn!(event = "suspend_timeout_expired", task_id = ..., queue = ..., "task auto-failed: suspended too long")`

- [x] Task 10: WorkerConfig.suspend_timeout field (AC: 4)
  - [x] 10.1 Add `suspend_timeout: Duration` to `WorkerConfig` in `crates/application/src/config.rs`
  - [x] 10.2 Default: `Duration::from_secs(24 * 60 * 60)` (24 hours)
  - [x] 10.3 Add validation: `suspend_timeout >= MIN_DURATION` in `WorkerConfig::validate()`
  - [x] 10.4 Add `#[serde(with = "humantime_serde")]` for config file support
  - [x] 10.5 Thread `suspend_timeout` from `WorkerConfig` → `SweeperService` constructor → `IronDefer::start()`

- [x] Task 11: Integration tests (AC: 1-5)
  - [x] 11.1 Create `crates/api/tests/suspend_test.rs`
  - [x] 11.2 Test: `suspend_transitions_to_suspended` — task handler calls `ctx.suspend()`, verify task status is `Suspended` and `suspended_at` is set
  - [x] 11.3 Test: `signal_resumes_suspended_task` — suspend task, call `POST /tasks/{id}/signal` with payload, verify task transitions to `Pending` and `signal_payload` is stored, verify on re-execution `ctx.signal_payload()` returns the data
  - [x] 11.4 Test: `concurrent_signals_exactly_one_wins` — suspend task, send 10 concurrent signal requests, verify exactly 1 returns 200 and the rest return 409
  - [x] 11.5 Test: `suspend_watchdog_auto_fails` — suspend task, set very short `suspend_timeout` (e.g., 1s), wait for sweeper tick, verify task status is `Failed` with `last_error = "suspend timeout exceeded"`
  - [x] 11.6 Test: `suspended_task_not_counted_in_concurrency` — suspend a task, verify another task can still be claimed (concurrency not blocked)
  - [x] 11.7 Test: `signal_on_non_suspended_returns_409` — try to signal a running/completed task, verify 409
  - [x] 11.8 Test: `signal_on_nonexistent_returns_404` — signal a random UUID, verify 404

- [x] Task 12: Offline cache & compilation (AC: all)
  - [x] 12.1 Regenerate `.sqlx/` offline cache
  - [x] 12.2 Verify `cargo test --workspace` passes
  
### Review Findings

- [x] [Review][Decision] Suspended Task Cancellation
- [x] [Review][Patch] Missing Suspend Timeout Metric
- [x] [Review][Patch] Missing Suspended Total Metric
- [x] [Review][Patch] Large Signal Payload Risk
- [x] [Review][Patch] "Forever Suspended" Zombie Risk
- [x] [Review][Patch] SQL "NULL" Hardcoding
- [x] [Review][Patch] Repetitive SQL Column Lists
- [x] [Review][Patch] Catch-all Status Match Arms
- [x] [Review][Patch] Brittle SQL Construction in `list_tasks`
- [x] [Review][Patch] Redundant Mapping Structs
- [x] [Review][Patch] Dead Branch Documentation
- [x] [Review][Defer] CLI Status Case Sensitivity [crates/api/src/http/handlers/tasks.rs] — deferred, pre-existing

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules:**
- Domain: `TaskContext.suspend()` returns a control-flow signal; `TaskContext.signal_payload()` accessor. No DB access in domain.
- Application (ports): `TaskRepository` gains `suspend()`, `signal()`, `expire_suspended_tasks()` methods
- Application (services): Worker dispatch handles suspend signal; Sweeper gains watchdog clause
- Infrastructure: `PostgresTaskRepository` implements the three new port methods with atomic SQL
- API: `POST /tasks/{id}/signal` endpoint + `SignalTaskRequest` DTO

**Critical design constraint — ctx.suspend() control flow:**
`ctx.suspend()` is called by the task handler inside `dispatch_task()`. The domain layer has no DB access. The approach is:
1. Handler calls `ctx.suspend()` which returns `Err(TaskError::SuspendRequested)` (or a dedicated sentinel)
2. `dispatch_task()` matches on this specific error variant in the `handler_result` match (line 656)
3. `dispatch_task()` calls `ctx.repo.suspend(task.id())` to do the actual DB UPDATE
4. Function returns — worker slot released by normal function exit

This keeps the domain layer clean (no DB dependencies) while allowing the handler to express "I want to suspend" declaratively.

### Critical Implementation Details

1. **Prerequisite — Story 12.0 must be complete.** The `tasks_status_check` CHECK constraint (migration 0001, line 26) restricts status to 5 values. Story 12.0's migration 0011 drops and recreates this constraint with `'suspended'` included. Without it, any `UPDATE tasks SET status = 'suspended'` will fail with a constraint violation.

2. **Suspend flow sequence:**
   - Handler calls `ctx.suspend()` (optionally with checkpoint data)
   - `ctx.suspend()` calls `self.checkpoint(data)` if data provided (G6 — persists state)
   - `ctx.suspend()` returns `Err(TaskError::SuspendRequested)` or equivalent signal
   - Worker dispatch intercepts signal, calls `repo.suspend(task_id)`
   - `repo.suspend()`: `UPDATE tasks SET status='suspended', suspended_at=now(), claimed_by=NULL, claimed_until=NULL WHERE id=$1 AND status='running'`
   - Function returns → worker slot released

3. **Signal flow sequence:**
   - `POST /tasks/{id}/signal` with `{ "payload": {...} }`
   - Handler calls `repo.signal(task_id, payload)`
   - `repo.signal()`: `UPDATE tasks SET status='pending', signal_payload=$1, suspended_at=NULL, scheduled_at=now() WHERE id=$2 AND status='suspended'`
   - Zero rows affected → 409 (task not suspended)
   - Next claim: worker picks up task, `TaskContext` has `signal_payload` from RETURNING clause
   - Handler reads `ctx.signal_payload()` to get approval data

4. **Concurrent signal safety:** The `WHERE status='suspended'` predicate in `signal()` provides natural atomic CAS. Only one concurrent UPDATE matches; the rest find the row already transitioned and return zero rows.

5. **Worker slot release:** No explicit semaphore release or connection release needed. The worker's `dispatch_task()` function returns normally, which releases:
   - The JoinSet entry (JoinHandle completes)
   - The semaphore permit (acquired at poll-loop level)
   - Any borrowed references from DispatchContext

6. **Sweeper interactions:**
   - Zombie recovery: `WHERE status='running'` — naturally excludes Suspended (already ensured by Story 12.0)
   - Suspend watchdog: NEW clause `WHERE status='suspended' AND suspended_at < now() - $timeout`
   - Idempotency cleanup: terminal-status predicate — Suspended is not terminal, so not affected

7. **Audit log:** If `audit_log=true`, `suspend()` inserts `running → suspended` audit row; `signal()` inserts `suspended → pending`; watchdog inserts `suspended → failed`. All wrapped in same transaction as the state UPDATE.

8. **OTel events:** `dispatch_task()` emits `task.state_transition` event for `running → suspended` with appropriate attributes.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `crates/domain/src/model/task.rs` | Add `ctx.suspend()`, `ctx.signal_payload()`, `SuspendRequested` error/signal, `signal_payload` field in TaskContext |
| `crates/domain/src/error.rs` | Add `TaskError::SuspendRequested` variant (or equivalent) |
| `crates/application/src/ports/task_repository.rs` | Add `suspend()`, `signal()`, `expire_suspended_tasks()` to trait + MockTaskRepository |
| `crates/application/src/services/worker.rs` | Handle suspend signal in `dispatch_task()` match, add `emit_task_suspended()` |
| `crates/application/src/services/sweeper.rs` | Add suspend watchdog clause, add `suspend_timeout` field |
| `crates/application/src/config.rs` | Add `suspend_timeout` to WorkerConfig |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Implement `suspend()`, `signal()`, `expire_suspended_tasks()` |
| `crates/api/src/http/handlers/tasks.rs` | Add `signal_task()` handler, `SignalTaskRequest` DTO |
| `crates/api/src/http/router.rs` | Register `POST /tasks/{id}/signal` route |
| `crates/api/src/lib.rs` | Thread `suspend_timeout` through IronDefer builder |
| `crates/api/tests/suspend_test.rs` | **NEW** — 7+ integration tests |
| `.sqlx/` | Regenerate offline cache |

### Testing Standards

- Integration tests in `crates/api/tests/suspend_test.rs` as flat file
- Use `fresh_pool_on_shared_container()` for clean DB state per test
- Unique queue names per test for isolation
- For concurrent signal test: use `tokio::sync::Barrier` to synchronize 10 tasks, assert exactly 1 success
- For watchdog test: configure short `suspend_timeout` (1-2s) and `sweeper_interval` (500ms)
- Skip gracefully when Docker is unavailable

### Previous Story Intelligence

**From Story 11.1 (checkpoint — TaskContext method addition):**
- `ctx.checkpoint()` uses `Arc<dyn CheckpointWriter>` trait for DB access — suspend should follow same hexagonal pattern
- `with_checkpoint()` builder method pattern for adding optional context to TaskContext
- Checkpoint writer is injected via `DispatchContext` — signal_payload should be loaded from TaskRecord at claim time

**From Story 11.3 (checkpoint E2E — test patterns):**
- `CheckpointStepTask` uses `fail_on` patterns with restricted `maxAttempts` to prove resume correctness
- `boot_e2e_engine_with_checkpoint()` registers custom task types — suspend tests need similar variant
- Serde unit struct fix: use `struct Foo {}` (empty struct), not `struct Foo;` (unit struct serializes as null)
- Sweeper race fix: handler sleeping past lease_duration races with retry. For suspend tests, use explicit synchronization

**From Story 10.2 (audit log — transaction wrapping):**
- State transitions with audit: `pool.begin()` → UPDATE task → INSERT audit → `tx.commit()`
- Same pattern applies to `suspend()`, `signal()`, and `expire_suspended_tasks()`

**From Story 6.3 (cancel — CTE atomicity):**
- Cancel uses WHERE guard for atomicity — same pattern for signal's `WHERE status='suspended'`
- Concurrent cancel test: 10 concurrent requests, exactly 1 success — same test pattern for signal

### References

- [Source: docs/artifacts/planning/epics.md — Epic 12, Story 12.1 (lines 1273-1308)]
- [Source: docs/artifacts/planning/prd.md — FR60-FR63 (lines 989-992), G7 spec (lines 193-199)]
- [Source: docs/artifacts/planning/architecture.md — G7 TaskContext.suspend() design, SuspendSignal, worker loop integration]
- [Source: crates/domain/src/model/task.rs — TaskContext (lines 307-403), checkpoint() method (line 372)]
- [Source: crates/application/src/services/worker.rs — dispatch_task() (lines 445-719), handler_result match (line 656)]
- [Source: crates/application/src/services/sweeper.rs — sweep tick (line 180), recover_zombie_tasks]
- [Source: crates/application/src/config.rs — WorkerConfig (line 56)]
- [Source: crates/application/src/ports/task_repository.rs — TaskRepository trait (line 26)]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — claim_next (line 622), complete (line 686)]
- [Source: crates/api/src/http/router.rs — route registration pattern (line 26)]

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

- All 12 tasks and subtasks completed
- TaskError::SuspendRequested control-flow variant added to domain error model
- TaskContext gains suspend() method (optionally checkpoints data, returns SuspendRequested sentinel)
- TaskContext gains signal_payload field and accessor, populated from TaskRecord at claim time
- TaskRepository port extended with suspend(), signal(), expire_suspended_tasks() methods
- PostgresTaskRepository implements all three with atomic SQL, audit log support, and 404/409 differentiation for signal()
- Worker dispatch_task() intercepts SuspendRequested — calls repo.suspend(), emits OTel event + lifecycle log, does NOT count as failure
- POST /tasks/{id}/signal REST endpoint with OpenAPI docs, SignalTaskRequest DTO
- Sweeper gains suspend watchdog — auto-fails tasks past suspend_timeout on each tick
- WorkerConfig.suspend_timeout field (default 24h) threaded through IronDefer::start() → SweeperService
- 7 integration tests: suspend transition, signal resume, concurrent signals (10-way race → exactly 1 wins), watchdog auto-fail, concurrency slot release, 409 on non-suspended, 404 on nonexistent
- sqlx offline cache regenerated
- All 78 unit tests + 7 suspend integration tests + 34 REST API tests + sweeper/worker tests pass
- Pre-existing flaky test e2e_trace_propagation_across_retries unrelated to this story

### Change Log

- 2026-04-25: Implemented HITL suspend/resume — 12 tasks, 7 integration tests, all ACs satisfied

### File List

- crates/domain/src/error.rs (modified — SuspendRequested variant)
- crates/domain/src/model/task.rs (modified — suspend(), signal_payload field/accessor, with_signal_payload builder)
- crates/application/src/ports/task_repository.rs (modified — suspend(), signal(), expire_suspended_tasks() methods)
- crates/application/src/services/worker.rs (modified — SuspendRequested match arm, emit_task_suspended, signal_payload population, StuckClaimRepo updated)
- crates/application/src/services/sweeper.rs (modified — suspend_timeout field, watchdog clause, with_suspend_timeout builder)
- crates/application/src/services/scheduler.rs (modified — signal() delegation method)
- crates/application/src/config.rs (modified — suspend_timeout field, validation, default)
- crates/infrastructure/src/adapters/postgres_task_repository.rs (modified — suspend(), signal(), expire_suspended_tasks() implementations)
- crates/api/src/http/handlers/tasks.rs (modified — SignalTaskRequest DTO, signal_task handler)
- crates/api/src/http/router.rs (modified — POST /tasks/{id}/signal route, OpenAPI registration)
- crates/api/src/http/errors.rs (modified — SuspendRequested → 409 mapping)
- crates/api/src/lib.rs (modified — signal() method, suspend_timeout threading to sweeper)
- crates/api/tests/suspend_test.rs (NEW — 7 integration tests)
- .sqlx/ (modified — regenerated offline cache)
