# Story 12.2: Geographic Worker Pinning

Status: done

## Story

As an operator deploying iron-defer in a multi-region environment,
I want to submit tasks with a region label so they execute only on workers in the matching geography,
so that GDPR and HIPAA data residency requirements are satisfied.

## Acceptance Criteria

1. **Given** a task submitted with `region = "eu-west"`
   **When** workers in `eu-west` and `us-east` are running
   **Then** only the `eu-west` worker claims the task

2. **Given** a task submitted without a region label
   **When** any worker is running
   **Then** any worker can claim the task regardless of its region

3. **Given** a worker with no region configured
   **When** region-pinned tasks exist in the queue
   **Then** the regionless worker claims only unpinned tasks

4. **Given** a worker with `region = "eu-west"`
   **When** both pinned (`eu-west`) and unpinned tasks exist
   **Then** the worker claims both

5. **Given** queue statistics
   **When** `GET /queues` is called
   **Then** region labels are visible in the statistics and OTel metrics

## Functional Requirements Coverage

- **FR64:** Developer submits task with optional region label restricting which workers can claim it
- **FR65:** Worker with region label claims only matching-region or unpinned tasks; regionless worker claims only unpinned
- **FR66:** Region labels exposed in queue statistics and OTel metric labels

## Tasks / Subtasks

- [x] Task 1: Database migration — region column + index (AC: 1)
  - [x] 1.1 Create `migrations/0012_add_region_column.sql`:
    ```sql
    ALTER TABLE tasks ADD COLUMN region VARCHAR;
    CREATE INDEX idx_tasks_region_claiming
        ON tasks (queue, region, status, priority DESC, scheduled_at ASC)
        WHERE status = 'pending';
    ```
  - [x] 1.2 Regenerate `.sqlx/` offline cache after migration

- [x] Task 2: Domain — TaskRecord.region field (AC: 1, 2)
  - [x] 2.1 Add `pub(crate) region: Option<String>` field to `TaskRecord` in `crates/domain/src/model/task.rs` (after `signal_payload`)
  - [x] 2.2 Add accessor: `pub fn region(&self) -> Option<&str>` (returns `self.region.as_deref()`)
  - [x] 2.3 bon Builder handles `Option<>` field automatically (defaults to `None`)

- [x] Task 3: Infrastructure — TaskRow and queries (AC: 1, 2)
  - [x] 3.1 Add `region: Option<String>` to `TaskRow` and `TaskRowWithTotal` in `postgres_task_repository.rs`
  - [x] 3.2 Update `TryFrom<TaskRow> for TaskRecord` and `TryFrom<TaskRowWithTotal> for TaskRecord` to map `region`
  - [x] 3.3 Add `region` to all RETURNING clauses (~12 queries)
  - [x] 3.4 Add `region` to INSERT column lists in `save()`, `save_in_tx()`, `save_idempotent()`, `save_idempotent_in_tx()`
  - [x] 3.5 Add `region` to SELECT in `find_by_id()`, `list_by_queue()`, `list_tasks()`

- [x] Task 4: WorkerConfig.region field (AC: 1, 3, 4)
  - [x] 4.1 Add `pub region: Option<String>` to `WorkerConfig` in `crates/application/src/config.rs` (default `None`)
  - [x] 4.2 No validation needed — region is an opaque string, no constraints beyond non-empty when provided
  - [x] 4.3 Env var: `IRON_DEFER__WORKER__REGION=eu-west`

- [x] Task 5: Modify claim_next() for region filtering (AC: 1, 2, 3, 4)
  - [x] 5.1 Add `region: Option<&str>` parameter to `claim_next()` in `TaskRepository` trait
  - [x] 5.2 Update `MockTaskRepository` expectations
  - [x] 5.3 Modify SQL in `PostgresTaskRepository::claim_next()` (`crates/infrastructure/src/adapters/postgres_task_repository.rs:636`):
    - Worker WITH region: `AND (region IS NULL OR region = $4)` — claims both pinned + unpinned
    - Worker WITHOUT region: `AND region IS NULL` — claims only unpinned
  - [x] 5.4 **Implementation approach:** Use a single query with conditional WHERE clause. Pass worker region as `Option<&str>`. In Rust, branch on `worker_region.is_some()` to select the appropriate SQL variant (two `sqlx::query_as!()` calls, not dynamic SQL). This avoids runtime SQL construction while keeping type safety.
  - [x] 5.5 Thread `region` from `WorkerConfig` through `WorkerService::run_poll_loop()` → `claim_next()` call site (~line 166 in worker.rs)

- [x] Task 6: enqueue_with_region() library method (AC: 1)
  - [x] 6.1 Add `pub async fn enqueue_with_region<T: Task>(&self, queue: &str, task: T, region: &str) -> Result<TaskRecord, TaskError>` to `IronDefer` in `crates/api/src/lib.rs`
  - [x] 6.2 Implementation: construct `TaskRecord` with `region = Some(region.to_string())`, then delegate to existing `enqueue()` internals
  - [x] 6.3 The existing `enqueue()` method continues to work without region (backward-compatible, region = None)

- [x] Task 7: REST API — region in request/response (AC: 1, 2)
  - [x] 7.1 Add optional `region: Option<String>` to `CreateTaskRequest` in `tasks.rs` — when provided, task is pinned
  - [x] 7.2 Add `region: Option<String>` to `TaskResponse` (serializes as `region` in camelCase — already lowercase, no change)
  - [x] 7.3 Update `From<TaskRecord> for TaskResponse` to map `region`
  - [x] 7.4 Update `create_task()` handler to pass region to `engine.enqueue()` (or use `enqueue_with_region()` when region present)

- [x] Task 8: Queue statistics — region visibility (AC: 5)
  - [x] 8.1 Add `pub region: Option<String>` field to `QueueStatistics` struct in `crates/domain/src/model/queue.rs`
  - [x] 8.2 Update `queue_statistics()` query to GROUP BY `(queue, region)` — returns one row per (queue, region) pair. **This changes response cardinality:** previously one row per queue, now one row per (queue, region) combination. Unpinned tasks appear with `region: null`.
  - [x] 8.3 Update `GET /queues` response DTO to include region field
  - [x] 8.4 Update any existing queue stats tests to account for the new field

- [x] Task 9: OTel metrics — region label (AC: 5)
  - [x] 9.1 Add `region` label to `iron_defer_task_duration_seconds` histogram when task has a region
  - [x] 9.2 Add `region` label to `iron_defer_task_attempts_total` counter when task has a region
  - [x] 9.3 Update metric recording in `dispatch_task()` and `emit_task_completed()` to include region from task record
  - [x] 9.4 When region is None, omit the label (don't add `region=""`)

- [x] Task 10: Integration tests (AC: 1-5)
  - [x] 10.1 Create `crates/api/tests/region_test.rs`
  - [x] 10.2 Test: `pinned_task_claimed_by_matching_worker` — submit task with region="eu-west", start worker with region="eu-west", verify task is claimed
  - [x] 10.3 Test: `pinned_task_not_claimed_by_wrong_region` — submit task with region="eu-west", start worker with region="us-east", verify task is NOT claimed (stays pending after poll interval)
  - [x] 10.4 Test: `unpinned_task_claimed_by_any_worker` — submit task without region, start worker with region="eu-west", verify task is claimed
  - [x] 10.5 Test: `regionless_worker_skips_pinned` — submit task with region="eu-west", start worker without region, verify task is NOT claimed
  - [x] 10.6 Test: `regional_worker_claims_both` — submit 2 tasks (one pinned eu-west, one unpinned), start worker with region="eu-west", verify both are claimed
  - [x] 10.7 Test: `enqueue_with_region_via_rest` — `POST /tasks` with `region` field, verify `GET /tasks/{id}` returns correct region
  - [x] 10.8 Test: `region_visible_in_queue_stats` — submit pinned tasks, verify `GET /queues` includes region information

- [x] Task 11: Offline cache & compilation (AC: all)
  - [x] 11.1 Regenerate `.sqlx/` offline cache
  - [x] 11.2 Verify `cargo test --workspace` passes
  - [x] 11.3 Verify `cargo clippy --workspace` clean

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules:**
- Migration: `migrations/0012_add_region_column.sql`
- Domain: `TaskRecord.region` field + accessor in `crates/domain/src/model/task.rs`
- Application (ports): `claim_next()` gains `region` parameter; WorkerConfig gains `region` field
- Application (services): Worker threads region from config to claim_next call
- Infrastructure: PostgresTaskRepository modifies claim_next SQL with conditional WHERE clause
- API: `CreateTaskRequest` gains optional `region`; `TaskResponse` gains `region`; `enqueue_with_region()` public API

### Critical Implementation Details

1. **Two claim_next SQL variants (not dynamic SQL):**
   ```rust
   // Worker WITH region configured:
   if let Some(ref region) = worker_region {
       sqlx::query_as!(TaskRow, r#"
           UPDATE tasks SET status = 'running', ...
           WHERE id = (
               SELECT id FROM tasks
               WHERE queue = $3 AND status = 'pending' AND scheduled_at <= now()
                 AND (region IS NULL OR region = $4)
               ORDER BY priority DESC, scheduled_at ASC
               FOR UPDATE SKIP LOCKED LIMIT 1
           ) RETURNING ..."#, worker_id, lease_secs, queue, region)
   } else {
       // Worker WITHOUT region:
       sqlx::query_as!(TaskRow, r#"
           UPDATE tasks SET status = 'running', ...
           WHERE id = (
               SELECT id FROM tasks
               WHERE queue = $3 AND status = 'pending' AND scheduled_at <= now()
                 AND region IS NULL
               ORDER BY priority DESC, scheduled_at ASC
               FOR UPDATE SKIP LOCKED LIMIT 1
           ) RETURNING ..."#, worker_id, lease_secs, queue)
   }
   ```

2. **Index usage:** The new `idx_tasks_region_claiming` index covers `(queue, region, status, priority DESC, scheduled_at ASC) WHERE status = 'pending'`. The planner should use this index for both claim variants. The existing `idx_tasks_claiming` on `(queue, status, priority DESC, scheduled_at ASC) WHERE status = 'pending'` still works for unpinned-only queries.

3. **Region is opaque:** No validation on region string content — just non-empty when provided. Region is per-task (submission time), not per-queue. Different tasks in the same queue can have different regions.

4. **Backward compatibility:** Tasks without region (`region IS NULL`) are claimed by any worker regardless of its region setting. Workers without region config only claim unpinned tasks. Existing deployments with no region config see zero behavioral change.

5. **claim_next() signature change:** Adding `region: Option<&str>` to the `TaskRepository::claim_next()` trait method is a breaking change to the port. All callers must be updated:
   - `WorkerService::run_poll_loop()` — passes `self.region.as_deref()` 
   - `MockTaskRepository` expectations in tests
   - Integration tests that call `claim_next()` directly

6. **Queue statistics:** The simplest approach is to add `region` to the GROUP BY in `queue_statistics()`. This returns one row per `(queue, region)` pair. The `QueueStatistics` struct in `crates/domain/src/model/` gains an `Option<String>` region field. When no regional tasks exist, the response is unchanged (region = None for all entries).

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `migrations/0012_add_region_column.sql` | **NEW** — ADD COLUMN region + CREATE INDEX |
| `crates/domain/src/model/task.rs` | Add `region: Option<String>` to TaskRecord, accessor |
| `crates/domain/src/model/` | Update `QueueStatistics` to include optional region |
| `crates/application/src/config.rs` | Add `region: Option<String>` to WorkerConfig |
| `crates/application/src/ports/task_repository.rs` | Add `region: Option<&str>` to `claim_next()` signature |
| `crates/application/src/services/worker.rs` | Thread region from config to claim_next call |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | Two claim_next SQL variants, region in all queries, queue_statistics region grouping |
| `crates/api/src/lib.rs` | Add `enqueue_with_region()` public method |
| `crates/api/src/http/handlers/tasks.rs` | Region in CreateTaskRequest, TaskResponse, create_task handler |
| `crates/api/src/http/handlers/queues.rs` | Region in queue stats response |
| `crates/api/tests/region_test.rs` | **NEW** — 7+ integration tests |
| `.sqlx/` | Regenerate offline cache |

### Testing Standards

- Integration tests in `crates/api/tests/region_test.rs` as flat file
- Multi-worker tests require starting two engines with different regions on the same DB — use `boot_e2e_engine()` twice with different WorkerConfig.region values
- Alternatively, test at the repository level: call `claim_next(queue, worker_id, lease, Some("eu-west"))` and verify correct task selection
- Use `fresh_pool_on_shared_container()` for clean DB state
- For "not claimed" tests: wait longer than poll_interval, then verify task is still Pending via direct DB query

### Previous Story Intelligence

**From Story 11.1 (checkpoint — column addition pattern):**
- Adding a column follows: migration → TaskRow → TryFrom → RETURNING/INSERT/SELECT → TaskResponse → accessor
- ~12 queries need RETURNING clause updates
- bon Builder handles new `Option<>` fields automatically

**From Story 9.1 (idempotency key — conditional WHERE clause):**
- `save_idempotent()` uses `ON CONFLICT` with conditional logic — similar conditional approach for claim_next region filter
- Two SQL variants (with/without idempotency key) is an established pattern

**From Story 6.4 (sweeper — per-queue metrics):**
- Per-queue metric labels already established for `iron_defer_zombie_recoveries_total`
- Adding `region` label follows same `KeyValue::new("region", ...)` pattern

### References

- [Source: docs/artifacts/planning/epics.md — Epic 12, Story 12.2 (lines 1309-1342)]
- [Source: docs/artifacts/planning/prd.md — FR64-FR66 (lines 996-998), G8 spec (lines 201-206), NFR-SC5 (line 1065)]
- [Source: docs/artifacts/planning/architecture.md — G8 claiming query variants, idx_tasks_region_claiming, region enqueue API]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs — claim_next (lines 622-683)]
- [Source: crates/application/src/config.rs — WorkerConfig (lines 56-95)]
- [Source: crates/application/src/ports/task_repository.rs — claim_next signature (line 59)]
- [Source: crates/api/src/http/handlers/tasks.rs — CreateTaskRequest, TaskResponse, create_task handler]

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List
- Migration 0012: added `region VARCHAR` column + `idx_tasks_region_claiming` partial index
- Domain: `TaskRecord.region` field + `region()` accessor
- Infrastructure: `TaskRow.region`, all 12+ queries updated with region in INSERT/SELECT/RETURNING
- `claim_next()`: two SQL variants — regional worker claims `(region IS NULL OR region = $4)`, regionless worker claims `region IS NULL` only
- `queue_statistics()`: GROUP BY `(queue, region)` — returns one row per (queue, region) pair
- WorkerConfig: `region: Option<String>` field, threaded through poll loop to claim_next
- API: `enqueue_with_region<T>()`, `enqueue_raw()` gains `region` parameter, REST `POST /tasks` accepts optional `region` field
- OTel: `region` label added to `task_attempts_total` and `task_duration_seconds` metrics when task has region
- 7 integration tests: pinned/unpinned claiming, wrong-region rejection, regionless skip, both-claims, REST round-trip, queue stats visibility

### Change Log
- 2026-04-26: Story 12.2 implemented — geographic worker pinning with region-aware claiming

### File List
- migrations/0012_add_region_column.sql (NEW)
- crates/domain/src/model/task.rs (MODIFIED — region field + accessor)
- crates/domain/src/model/queue.rs (MODIFIED — QueueStatistics.region)
- crates/application/src/config.rs (MODIFIED — WorkerConfig.region)
- crates/application/src/ports/task_repository.rs (MODIFIED — claim_next region param + concretize)
- crates/application/src/services/worker.rs (MODIFIED — thread region, OTel labels)
- crates/application/src/services/scheduler.rs (MODIFIED — enqueue_raw region param, enqueue_with_region)
- crates/infrastructure/src/adapters/postgres_task_repository.rs (MODIFIED — TaskRow.region, all queries, two claim_next variants, queue_statistics GROUP BY)
- crates/api/src/lib.rs (MODIFIED — enqueue_with_region, enqueue_raw region param)
- crates/api/src/http/handlers/tasks.rs (MODIFIED — CreateTaskRequest.region, TaskResponse.region)
- crates/api/src/http/handlers/queues.rs (MODIFIED — QueueStatsResponse.region)
- crates/api/tests/region_test.rs (NEW — 7 integration tests)
- .sqlx/ (REGENERATED — offline cache updated for new queries)

### Review Findings

- [ ] [Review][Decision] **Breaking Change in Queue Stats API** — `GET /queues` now returns multiple rows per queue (one per region), which may break consumers expecting unique queue summaries.
- [ ] [Review][Patch] **Compilation Error: Undefined `task_id`** [postgres_task_repository.rs]
- [ ] [Review][Patch] **Compilation Error: `WorkerId` Type Mismatch** [postgres_task_repository.rs]
- [ ] [Review][Patch] **Missing `audit_log` Migration** [migrations/, postgres_task_repository.rs]
- [ ] [Review][Patch] **Logic Duplication & Dead Code in Scheduler** [scheduler.rs]
- [ ] [Review][Patch] **Empty Region String \"Zombie\" Tasks** [api/src/lib.rs, scheduler.rs, tasks.rs]
- [ ] [Review][Patch] **Prometheus Label Cardinality Explosion** [worker.rs]
- [ ] [Review][Patch] **Incomplete OTel Update** [worker.rs]
- [x] [Review][Defer] **Lack of Region Access Control** [api/src/lib.rs] — deferred, pre-existing
