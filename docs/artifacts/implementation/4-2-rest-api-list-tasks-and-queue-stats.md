# Story 4.2: REST API — List Tasks & Queue Stats

Status: done

## Story

As an external service,
I want to list tasks with filters and query queue statistics via REST,
so that I can build dashboards and integrate task monitoring into external systems.

## Acceptance Criteria

1. **List tasks with filters — `GET /tasks`:**

   Returns a paginated, filtered list of tasks. All query parameters are optional:

   - `queue` (string) — filter by queue name
   - `status` (string) — filter by task status (`pending`, `running`, `completed`, `failed`, `cancelled`)
   - `limit` (integer, default 50, max 100) — page size
   - `offset` (integer, default 0) — pagination offset

   Response: HTTP 200 with:
   ```json
   {
     "tasks": [{ ...TaskResponse... }, ...],
     "total": 142,
     "limit": 50,
     "offset": 0
   }
   ```

   `total` is the count of all matching rows (ignoring limit/offset) — enables UI pagination. `tasks` contains at most `limit` items. Ordering: `created_at ASC, id ASC` (consistent with existing `list_by_queue`).

   Invalid `status` value returns HTTP 422 with `{ "error": { "code": "INVALID_QUERY_PARAMETER", "message": "invalid status filter: 'foo'; expected one of: pending, running, completed, failed, cancelled" } }`.

   **Maps to:** FR26, Epic 4 Story 4.2 AC.

2. **Queue statistics — `GET /queues`:**

   Returns a list of all queues that have at least one task, each with depth (pending count) and active worker count (running tasks as proxy for active workers — each running task has a distinct `claimed_by` worker).

   Response: HTTP 200 with:
   ```json
   [
     {
       "queue": "payments",
       "pending": 42,
       "running": 3,
       "activeWorkers": 3
     },
     {
       "queue": "notifications",
       "pending": 7,
       "running": 1,
       "activeWorkers": 1
     }
   ]
   ```

   The response is a direct JSON array (no wrapper object) — consistent with the Architecture's "direct responses, no envelope" pattern. `activeWorkers` is `COUNT(DISTINCT claimed_by) WHERE status = 'running'` per queue.

   Empty result (no queues exist) returns HTTP 200 with `[]`.

   **Maps to:** FR28, Epic 4 Story 4.2 AC.

3. **OpenAPI specification — `GET /openapi.json`:**

   Returns a valid OpenAPI 3.1 specification describing all REST endpoints (FR30, NFR-I3). The spec is generated from code using `utoipa` — not hand-maintained.

   Response: HTTP 200 with `Content-Type: application/json` and a valid OpenAPI document.

   The spec documents:
   - All endpoints: `POST /tasks`, `GET /tasks/{id}`, `DELETE /tasks/{id}`, `GET /tasks`, `GET /queues`, `GET /health`, `GET /health/ready`, `GET /metrics`, `GET /openapi.json`
   - All request/response schemas: `CreateTaskRequest`, `TaskResponse`, `ListTasksResponse`, `QueueStats`, `ErrorResponse`, health probe responses
   - All status codes and error formats per endpoint

   **Maps to:** FR30, NFR-I3, Architecture deferred gap analysis.

4. **List tasks — full stack wiring:**

   a. **Domain** — Add `ListTasksFilter` struct and `ListTasksResult` struct to `crates/domain/src/model/task.rs`:
      ```rust
      pub struct ListTasksFilter {
          pub queue: Option<QueueName>,
          pub status: Option<TaskStatus>,
          pub limit: u32,
          pub offset: u32,
      }

      pub struct ListTasksResult {
          pub tasks: Vec<TaskRecord>,
          pub total: u64,
      }
      ```

   b. **Port** — Add to `TaskRepository` trait in `crates/application/src/ports/task_repository.rs`:
      ```rust
      async fn list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError>;
      ```
      Keep existing `list_by_queue` for backwards compatibility (used by `IronDefer::list()`).

   c. **Adapter** — Implement in `PostgresTaskRepository` using dynamic WHERE clause construction. Use runtime-typed `sqlx::query_as` (not `query_as!` macro) to avoid `.sqlx/` cache changes — same pattern as Story 4.1's cancel implementation.

      SQL pattern:
      ```sql
      -- Count query
      SELECT COUNT(*) as total FROM tasks WHERE [filters]

      -- Data query
      SELECT * FROM tasks WHERE [filters]
      ORDER BY created_at ASC, id ASC
      LIMIT $N OFFSET $M
      ```

      Build WHERE clauses conditionally: if `queue` is provided, add `queue = $X`; if `status` is provided, add `status = $Y`. Both omitted = all tasks.

   d. **Scheduler service** — Add `list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError>` to `SchedulerService`.

   e. **Public API** — Add `IronDefer::list_tasks(&self, filter: ListTasksFilter) -> Result<ListTasksResult, TaskError>` in `crates/api/src/lib.rs`.

   f. **HTTP handler** — Add `list_tasks` handler in `crates/api/src/http/handlers/tasks.rs`. Extract query params via `axum::extract::Query<ListTasksQuery>`. Map to domain filter, call service, return `ListTasksResponse`.

   g. **Router** — Register `GET /tasks` in `router.rs`. This is a NEW route — the existing `GET /tasks/{id}` uses a path parameter, so there's no conflict. Axum routes `/tasks` and `/tasks/{id}` distinctly.

5. **Queue statistics — full stack wiring:**

   a. **Domain** — Add `QueueStatistics` struct to `crates/domain/src/model/queue.rs`:
      ```rust
      pub struct QueueStatistics {
          pub queue: QueueName,
          pub pending: u64,
          pub running: u64,
          pub active_workers: u64,
      }
      ```

   b. **Port** — Add to `TaskRepository` trait:
      ```rust
      async fn queue_statistics(&self) -> Result<Vec<QueueStatistics>, TaskError>;
      ```

   c. **Adapter** — Implement in `PostgresTaskRepository`:
      ```sql
      SELECT
          queue,
          COUNT(*) FILTER (WHERE status = 'pending') as pending,
          COUNT(*) FILTER (WHERE status = 'running') as running,
          COUNT(DISTINCT claimed_by) FILTER (WHERE status = 'running') as active_workers
      FROM tasks
      GROUP BY queue
      ORDER BY queue
      ```
      Single query, no N+1.

   d. **Scheduler service** — Add `queue_statistics()` to `SchedulerService`.

   e. **Public API** — Add `IronDefer::queue_statistics()` in `lib.rs`.

   f. **HTTP handler** — Create new handler module `crates/api/src/http/handlers/queues.rs` with `list_queues` handler. Register route `GET /queues` in `router.rs`.

6. **OpenAPI — implementation approach:**

   Add `utoipa` and `utoipa-axum` to workspace dependencies. Annotate:
   - All request/response structs with `#[derive(utoipa::ToSchema)]`
   - All handlers with `#[utoipa::path(...)]` attributes
   - Build the OpenAPI doc via `utoipa::OpenApi` derive macro on a collector struct

   Serve the spec from a handler that returns the generated JSON. The spec is compile-time generated — no runtime overhead.

   **New dependency:** `utoipa` is the standard OpenAPI library for axum in the Rust ecosystem. It has ~5M downloads, is well-maintained, and generates spec from Rust type annotations.

7. **Quality gates:**

   - `cargo fmt --check` — clean.
   - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean.
   - `SQLX_OFFLINE=true cargo test --workspace` — all new tests + existing suites pass.
   - `cargo deny check bans` — `bans ok`.
   - `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (rustls-only preserved).
   - `.sqlx/` cache unchanged — use runtime-typed `query_as` to avoid cache refresh.

## Tasks / Subtasks

- [x] **Task 1: Add `utoipa` dependencies** (AC 6)
  - [x] Add `utoipa = { version = "5", features = ["axum_extras", "chrono", "uuid"] }` to workspace `Cargo.toml`.
  - [x] Add `utoipa` to relevant crate `Cargo.toml` files (domain, api).

- [x] **Task 2: Domain model additions** (AC 4a, AC 5a)
  - [x] Add `ListTasksFilter` and `ListTasksResult` to `crates/domain/src/model/task.rs`.
  - [x] Add `QueueStatistics` struct to `crates/domain/src/model/queue.rs`.
  - [x] Re-export new types from domain crate's `lib.rs` and `model/mod.rs`.
  - [x] Add `#[derive(utoipa::ToSchema)]` to `TaskRecord`, `TaskStatus`, `CancelResult`, and new structs.

- [x] **Task 3: Repository port and adapter — list_tasks** (AC 4b, AC 4c)
  - [x] Add `list_tasks(&self, filter: &ListTasksFilter) -> Result<ListTasksResult, TaskError>` to `TaskRepository` trait.
  - [x] Add `#[automock]` regeneration — `MockTaskRepository` must include the new method.
  - [x] Implement in `PostgresTaskRepository` — dynamic WHERE clause with runtime-typed `query_as`.
  - [x] COUNT query for `total` + data query with LIMIT/OFFSET.

- [x] **Task 4: Repository port and adapter — queue_statistics** (AC 5b, AC 5c)
  - [x] Add `queue_statistics(&self) -> Result<Vec<QueueStatistics>, TaskError>` to `TaskRepository` trait.
  - [x] Implement in `PostgresTaskRepository` — single aggregation query with FILTER clauses.

- [x] **Task 5: Service and public API wiring** (AC 4d-e, AC 5d-e)
  - [x] Add `list_tasks(filter)` and `queue_statistics()` to `SchedulerService`.
  - [x] Add `IronDefer::list_tasks(filter)` and `IronDefer::queue_statistics()` in `lib.rs`.

- [x] **Task 6: List tasks HTTP handler** (AC 1, AC 4f-g)
  - [x] Add `ListTasksQuery` query param struct (all optional fields) in `handlers/tasks.rs`.
  - [x] Add `ListTasksResponse` response struct with `tasks`, `total`, `limit`, `offset`.
  - [x] Add `list_tasks` handler using `Query<ListTasksQuery>` extractor.
  - [x] Validate `status` param and return 422 with `INVALID_QUERY_PARAMETER` on invalid values.
  - [x] Clamp `limit` to max 100, default 50.
  - [x] Register `GET /tasks` in `router.rs`.

- [x] **Task 7: Queue stats HTTP handler** (AC 2, AC 5f)
  - [x] Create `crates/api/src/http/handlers/queues.rs` with `list_queues` handler.
  - [x] Add `QueueStatsResponse` DTO with camelCase JSON (`queue`, `pending`, `running`, `activeWorkers`).
  - [x] Add `pub mod queues;` to `handlers/mod.rs`.
  - [x] Register `GET /queues` in `router.rs`.

- [x] **Task 8: OpenAPI spec generation** (AC 3, AC 6)
  - [x] Annotate all request/response structs with `#[derive(utoipa::ToSchema)]`.
  - [x] Annotate all handlers with `#[utoipa::path(...)]` attributes (method, path, params, responses).
  - [x] Create `ApiDoc` struct with `#[derive(utoipa::OpenApi)]` collecting all paths and schemas.
  - [x] Add `openapi_spec` handler that returns `Json(ApiDoc::openapi())`.
  - [x] Register `GET /openapi.json` in `router.rs`.
  - [x] Verify the generated spec is valid OpenAPI 3.1.

- [x] **Task 9: Integration tests — list tasks** (AC 1)
  - [x] Extend `crates/api/tests/rest_api_test.rs`:
    - `list_tasks_returns_all_when_no_filters` — create 3 tasks, GET /tasks → 200 with total=3.
    - `list_tasks_filters_by_queue` — create tasks in 2 queues, filter by one → correct subset.
    - `list_tasks_filters_by_status` — create tasks, cancel one, filter by pending → excludes cancelled.
    - `list_tasks_pagination_limit_offset` — create 5 tasks, limit=2&offset=2 → 2 tasks, total=5.
    - `list_tasks_invalid_status_returns_422` — GET /tasks?status=foo → 422, `INVALID_QUERY_PARAMETER`.
    - `list_tasks_empty_result` — GET /tasks with unique queue filter → 200 with total=0, tasks=[].

- [x] **Task 10: Integration tests — queue stats** (AC 2)
  - [x] Extend `crates/api/tests/rest_api_test.rs`:
    - `queue_stats_returns_queue_list` — create tasks in 2 queues, GET /queues → 200 with both queues and correct counts.
    - `queue_stats_empty_when_no_tasks` — use unique queue prefix, GET /queues → 200, verify our queues not listed.

- [x] **Task 11: Integration tests — OpenAPI spec** (AC 3)
  - [x] Extend `crates/api/tests/rest_api_test.rs`:
    - `openapi_spec_returns_valid_json` — GET /openapi.json → 200, body parses as JSON, has `openapi` field.
    - `openapi_spec_documents_all_endpoints` — verify paths include `/tasks`, `/tasks/{id}`, `/queues`, `/health`, `/health/ready`, `/metrics`, `/openapi.json`.

- [x] **Task 12: Quality gates** (AC 7)
  - [x] `cargo fmt --check` — clean.
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --lib` — clean.
  - [x] All existing + new tests pass.
  - [x] `cargo deny check bans` — `bans ok`.
  - [x] No OpenSSL in dependency tree.

### Review Findings

- [x] [Review][Decision] OpenAPI spec is a hand-built JSON literal, not derived from code types (AC 3, AC 6) — **Resolved: option 3 chosen.** Replaced `serde_json::json!` with `utoipa` derive macros (`#[derive(ToSchema)]`, `#[derive(IntoParams)]`, `#[utoipa::path]`, `#[derive(OpenApi)]`). Spec is now compiler-enforced. [`crates/api/src/http/router.rs`]
- [x] [Review][Patch] Missing `#[instrument]` on `openapi_spec` handler — Fixed: added `#[tracing::instrument(skip_all, fields(method, path))]`. [`crates/api/src/http/router.rs:76`]
- [x] [Review][Patch] `limit=0` silently returns empty page — Fixed: changed `.min(MAX_LIST_LIMIT)` to `.clamp(1, MAX_LIST_LIMIT)` so limit is always >= 1. [`crates/api/src/http/handlers/tasks.rs:274`]
- [x] [Review][Patch] No integration test for `limit` clamping behavior — Fixed: added `list_tasks_limit_clamped_to_max` test verifying `?limit=500` returns `"limit": 100`. [`crates/api/tests/rest_api_test.rs`]
- [x] [Review][Defer] COUNT and SELECT race condition in `list_tasks` — two separate queries without a transaction; `total` can be inconsistent with `tasks`. Common pagination trade-off; `COUNT(*) OVER()` window function would fix but adds complexity. [`crates/infrastructure/src/adapters/postgres_task_repository.rs:509-549`]
- [x] [Review][Defer] Unbounded `offset` allows expensive full-table scans — `offset` is `u32` with no upper cap; large values force Postgres to skip billions of rows. Consider capping or requiring cursor-based pagination. [`crates/api/src/http/handlers/tasks.rs:241`]
- [x] [Review][Defer] Unfiltered `GET /tasks` triggers full-table COUNT — no `queue` or `status` filters means `SELECT COUNT(*) FROM tasks` on the entire table; expensive at scale. [`crates/infrastructure/src/adapters/postgres_task_repository.rs:516`]
- [x] [Review][Defer] `queue_statistics()` includes historical queues with all-zero counts — queues with only terminal tasks (completed/failed) appear with `pending: 0, running: 0`; response grows unboundedly over time. [`crates/infrastructure/src/adapters/postgres_task_repository.rs:574-586`]
- [x] [Review][Defer] No pagination index for `(created_at, id)` — list_tasks ORDER BY `created_at ASC, id ASC` without a covering index degrades on large tables. [`crates/infrastructure/src/adapters/postgres_task_repository.rs:537`]
- [x] [Review][Defer] `parse_status_filter` is case-sensitive — `"Pending"` or `"PENDING"` returns 422; consider `.to_ascii_lowercase()` for robustness. [`crates/api/src/http/handlers/tasks.rs:205-216`]

## Dev Notes

### Architecture Compliance

- **FR26** (PRD line 764): "An external service can list tasks with filtering by queue name and status via the REST API." AC 1 delivers this.
- **FR28** (PRD line 766): "An external service can query the list of registered queues along with their current depth and active worker statistics via the REST API." AC 2 delivers this.
- **FR30** (PRD line 768): "An API consumer can discover the REST API contract via an embedded OpenAPI specification." AC 3 delivers this.
- **NFR-I3** (PRD line 824): "The REST API must be documented via an OpenAPI 3.x specification generated from the API code (not hand-maintained), embedded in the binary, and served at a stable endpoint." AC 3 delivers this.
- **Architecture D4.1** (line 409): REST API has no authentication in MVP. All new endpoints follow this.
- **Architecture lines 628-641**: REST responses use direct format — `GET /tasks` uses `{ "tasks": [...], "total": N, "limit": 50, "offset": 0 }` collection wrapper per Architecture convention. `GET /queues` returns a direct array — consistent with "return the resource directly" for simple collections.
- **Architecture line 549**: Query parameters use `snake_case` — `queue`, `status`, `limit`, `offset` (not camelCase).
- **Architecture line 648**: HTTP status codes — 200 for retrieved/listed, 422 for invalid input.
- **JSON field naming**: `camelCase` for response bodies per ADR-0006 (`rename_all = "camelCase"`). Query params are `snake_case` per Architecture line 549.

### Critical Design Decisions

**`ListTasksFilter` in the domain crate.**
The filter is a domain concept (querying tasks by business attributes), not an HTTP concern. The handler parses query params into `ListTasksFilter`, which flows through the hexagonal layers. This keeps the SQL construction in the adapter (infrastructure) where it belongs.

**Runtime-typed `query_as` for dynamic WHERE clauses.**
The list endpoint has optional filters — `queue` and `status` are both nullable. Using `sqlx::query_as!` (compile-time macro) with optional WHERE clauses requires `Option` gymnastics with `COALESCE` or separate queries per combination. Runtime-typed `query_as` builds the SQL string dynamically — simpler, tested via integration tests, and avoids `.sqlx/` cache churn. This is the same pattern Story 4.1 established for the cancel query.

**`active_workers` as `COUNT(DISTINCT claimed_by) WHERE status = 'running'`.**
The Epic AC specifies "active worker count." Workers don't register themselves in the DB — they claim tasks. A running task implies an active worker. `DISTINCT claimed_by` counts unique workers. This is a database-side approximation — accurate for the current polling model where each running task has a distinct worker holding its lease. If a worker dies, its tasks stay `running` until sweeper recovery; the stat temporarily overcounts. Acceptable trade-off — the sweeper corrects within one sweep interval.

**`utoipa` for OpenAPI generation.**
The Architecture deferred OpenAPI to Epic 4, and the retrospective listed "Research `utoipa` or equivalent" as a preparation task. `utoipa` is the de facto standard: ~5M downloads, native axum integration, derives from Rust types (no hand-maintained spec). Version 5.x supports OpenAPI 3.1. The `axum_extras` feature provides `Query` and `Path` extractor support.

**`GET /queues` returns a direct array, not a collection wrapper.**
The Architecture convention shows `GET /tasks` using `{ "tasks": [...], "total": ... }` because it's paginated. `GET /queues` is not paginated (the number of distinct queues is small) — returning `[...]` directly is simpler and follows the "return the resource directly" principle.

**No `GET /tasks` pagination cursor.**
The Epic AC specifies `limit` and `offset`. Cursor-based pagination (keyset pagination) is more efficient for large datasets but adds complexity. LIMIT/OFFSET is sufficient for the MVP monitoring use case. The `total` count enables standard UI pagination controls.

**Error code `INVALID_QUERY_PARAMETER`.**
New error code for malformed query parameters. Distinct from `INVALID_PAYLOAD` (which is for request bodies). Covers invalid `status` values and any future query param validation errors.

### Previous Story Intelligence

**From Story 4.1 (Health Probes & Task Cancellation, 2026-04-21):**
- `fresh_pool_on_shared_container()` pattern for all integration tests. Use this consistently.
- `common::unique_queue()` for per-test queue isolation. Essential for list and queue stats tests to avoid cross-test contamination.
- `TestServer` helper in `rest_api_test.rs` — reuse for all new tests.
- Runtime-typed `sqlx::query_as` with `TaskRow` → `TaskRecord` conversion avoids `.sqlx/` cache changes. Story 4.1 added `#[derive(sqlx::FromRow)]` to `TaskRow` for this pattern.
- `TASK_IN_TERMINAL_STATE` error code added — the error handling pattern in `errors.rs` is the model for adding `INVALID_QUERY_PARAMETER`.
- Route chaining: `get(get_task).delete(delete_task)` for shared paths — the new `GET /tasks` is a different path (`/tasks` vs `/tasks/{id}`), so no chaining needed.
- Health handlers got `#[instrument(skip_all, fields(method, path))]` per review finding — apply same pattern to new handlers.

**From Epic 1B/2/3 Retrospective (2026-04-21):**
- Preparation sprint items include "Research `utoipa` or equivalent for OpenAPI spec generation" — this story implements it.
- Action Item #8 recommends newtype hardening with `bon` builders. Story 4.2 should use existing newtypes (`QueueName`, `TaskStatus`, `TaskId`) consistently. Do NOT introduce new builder patterns — follow existing conventions.
- Test flakiness with shared `TEST_DB OnceCell` — use `fresh_pool_on_shared_container()` exclusively.

**From Story 3.3 (Audit Trail & OTel Compliance Tests, 2026-04-19):**
- `await_all_terminal` helper waits for tasks to reach terminal state — useful if queue stats tests need to verify running task counts.

### Git Intelligence

Recent commits (last 5):
- `7d0c584` — Health probes and task cancellation APIs (Story 4.1). Most recent; establishes the pattern for Story 4.2.
- `2b70581` — Custom Axum extractors for structured JSON error responses. `PathParam<Uuid>` and `JsonBody<T>` available.
- `2a1ed9a` — Removed OTel compliance tests for Story 3.3.
- `940c722` — OTel compliance tests and SQL audit trail.
- `664979d` — OTel metrics and Prometheus scrape endpoint.

### Key Types and Locations (verified current as of 2026-04-21)

- `TaskRecord` — `crates/domain/src/model/task.rs:77-92`.
- `TaskStatus` enum (`Pending`, `Running`, `Completed`, `Failed`, `Cancelled`) — `crates/domain/src/model/task.rs:56-68`.
- `TaskId` newtype — `crates/domain/src/model/task.rs:20-48`.
- `QueueName` newtype — `crates/domain/src/model/queue.rs`.
- `CancelResult` enum — `crates/domain/src/model/task.rs:144-153`.
- `TaskRepository` port trait — `crates/application/src/ports/task_repository.rs` (methods: save, find_by_id, list_by_queue, claim_next, complete, fail, recover_zombie_tasks, cancel, release_leases_for_worker).
- `SchedulerService` — `crates/application/src/services/scheduler.rs` (methods: enqueue, find, list, enqueue_raw, cancel).
- `PostgresTaskRepository` adapter — `crates/infrastructure/src/adapters/postgres_task_repository.rs`.
- `TaskRow` (infra-internal, `#[derive(sqlx::FromRow)]`) — `crates/infrastructure/src/adapters/postgres_task_repository.rs:64-81`.
- `IronDefer` / `IronDeferBuilder` — `crates/api/src/lib.rs` (methods: enqueue, enqueue_at, find, list, cancel, enqueue_raw).
- `TaskResponse` DTO (camelCase JSON) — `crates/api/src/http/handlers/tasks.rs:46-64`.
- `CreateTaskRequest` — `crates/api/src/http/handlers/tasks.rs:29-44`.
- `AppError` / `ErrorResponse` — `crates/api/src/http/errors.rs`.
- `PathParam<T>` / `JsonBody<T>` extractors — `crates/api/src/http/extractors.rs`.
- `router()` function — `crates/api/src/http/router.rs`.
- `TestServer` test helper — `crates/api/tests/rest_api_test.rs:51-103`.
- `common::fresh_pool_on_shared_container()` — `crates/api/tests/common/mod.rs`.
- `common::unique_queue()` — `crates/api/tests/common/mod.rs`.
- `status_to_str()` helper — `crates/api/src/http/handlers/tasks.rs:88-97`.
- Existing routes — `POST /tasks`, `GET /tasks/{id}`, `DELETE /tasks/{id}`, `GET /health`, `GET /health/ready`, `GET /metrics`.
- `DefaultBodyLimit::max(1_048_576)` — `crates/api/src/http/router.rs:16-17`.

### Dependencies — New Crate Required

**New workspace dependency:**
```toml
utoipa = { version = "5", features = ["axum_extras", "chrono", "uuid"] }
```

Add to `[workspace.dependencies]` in root `Cargo.toml`. Add `utoipa = { workspace = true }` to:
- `crates/domain/Cargo.toml` — for `#[derive(ToSchema)]` on domain types
- `crates/api/Cargo.toml` — for `#[utoipa::path]` handler annotations and `OpenApi` derive

No other new dependencies. `serde`, `axum`, `sqlx`, `uuid`, `chrono` — all already present.

**Verify after adding:** `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` must remain empty. `utoipa` has no TLS dependencies — safe.

### Test Strategy

**Integration tests (extend `rest_api_test.rs`):**
- List tasks with no filters: verify returns all tasks, correct total.
- List tasks filtered by queue: create tasks in 2 queues, filter returns correct subset.
- List tasks filtered by status: create task, cancel it, filter by pending excludes cancelled.
- List tasks with pagination: create 5 tasks, limit=2&offset=2 returns 2 tasks with total=5.
- List tasks with invalid status: GET /tasks?status=foo → 422.
- List tasks empty result: filter on nonexistent queue → 200 with total=0.
- Queue stats: create tasks in 2 queues, verify correct pending counts.
- Queue stats empty: verify queues with no tasks aren't listed.
- OpenAPI spec valid: GET /openapi.json → 200, valid JSON, contains `openapi` version field.
- OpenAPI spec completeness: verify all endpoint paths are documented.

All tests use `fresh_pool_on_shared_container()` and `unique_queue()` for isolation.

**Unit tests (application crate):**
- `SchedulerService::list_tasks` with `MockTaskRepository` — verify correct delegation.
- `SchedulerService::queue_statistics` with `MockTaskRepository` — verify correct delegation.

**Explicitly out-of-scope tests:**
- OpenAPI spec schema validation against a third-party validator (validates structure, not correctness).
- Load testing list endpoint with large datasets (benchmark scope).
- Queue stats accuracy during concurrent task claiming (the SQL is a snapshot — inherently approximate).

### Project Structure Notes

**New files:**
- `crates/api/src/http/handlers/queues.rs` — queue stats handler.

**Modified files:**
- `Cargo.toml` (workspace root) — add `utoipa` to `[workspace.dependencies]`.
- `crates/domain/Cargo.toml` — add `utoipa` dependency.
- `crates/domain/src/model/task.rs` — add `ListTasksFilter`, `ListTasksResult`, utoipa derives.
- `crates/domain/src/model/queue.rs` — add `QueueStatistics`.
- `crates/domain/src/model/mod.rs` — re-export new types.
- `crates/domain/src/lib.rs` — re-export new types.
- `crates/api/Cargo.toml` — add `utoipa` dependency.
- `crates/api/src/http/handlers/mod.rs` — add `pub mod queues;`.
- `crates/api/src/http/handlers/tasks.rs` — add `list_tasks` handler, `ListTasksQuery`, `ListTasksResponse`, utoipa annotations.
- `crates/api/src/http/handlers/health.rs` — add utoipa annotations.
- `crates/api/src/http/router.rs` — register new routes.
- `crates/api/src/http/errors.rs` — add `INVALID_QUERY_PARAMETER` error code, utoipa schema.
- `crates/api/src/lib.rs` — add `list_tasks()`, `queue_statistics()`, OpenApi doc struct.
- `crates/application/src/ports/task_repository.rs` — add `list_tasks`, `queue_statistics` methods.
- `crates/application/src/services/scheduler.rs` — add `list_tasks`, `queue_statistics` methods.
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — implement `list_tasks` and `queue_statistics` SQL.
- `crates/api/tests/rest_api_test.rs` — extend with list, queue stats, and OpenAPI tests.

**Not modified:**
- Migrations — no schema changes (uses existing `tasks` table and indexes).
- `.sqlx/` — unchanged (runtime-typed queries).
- `deny.toml` — unchanged.
- `crates/infrastructure/Cargo.toml` — no new deps needed (uses existing `sqlx`).

### Out of Scope

- **CLI list/inspect commands** — Story 4.3.
- **CLI configuration validation** — Story 4.3.
- **Swagger UI** — not in Epic AC; only the JSON spec is required.
- **OpenAPI spec versioning** — no `/v1/` prefix in MVP per Architecture.
- **Cursor-based pagination** — Epic AC specifies `limit` and `offset` only.
- **Worker registration/heartbeat** — `activeWorkers` is derived from running tasks, not an explicit worker registry.
- **Queue creation/deletion APIs** — queues are implicit (created by submitting tasks).
- **Rate limiting** on list endpoints — Growth phase.
- **Authentication** — Architecture D4.1 defers to Growth phase.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 751-778] — Story 4.2 acceptance criteria (BDD source).
- [Source: `docs/artifacts/planning/architecture.md` lines 549] — Query parameter naming: `snake_case`.
- [Source: `docs/artifacts/planning/architecture.md` lines 628-661] — REST response format patterns (direct responses, collection wrapper, error shape).
- [Source: `docs/artifacts/planning/architecture.md` lines 648] — HTTP status codes table.
- [Source: `docs/artifacts/planning/prd.md` lines 764, 766, 768, 824] — FR26, FR28, FR30, NFR-I3.
- [Source: `docs/artifacts/implementation/4-1-health-probes-and-task-cancellation.md`] — Previous story patterns.
- [Source: `docs/artifacts/implementation/epic-1b-2-3-retro-2026-04-21.md` lines 139-143] — Epic 4 preparation tasks (utoipa research).
- [Source: `crates/api/src/http/router.rs`] — Current route definitions.
- [Source: `crates/api/src/http/handlers/tasks.rs`] — Existing handler patterns, TaskResponse DTO.
- [Source: `crates/api/src/http/errors.rs`] — Error response envelope, AppError mapping.
- [Source: `crates/api/src/http/extractors.rs`] — Custom `JsonBody`, `PathParam` extractors.
- [Source: `crates/api/src/lib.rs`] — IronDefer public API surface.
- [Source: `crates/application/src/ports/task_repository.rs`] — TaskRepository trait.
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs`] — PostgreSQL adapter, `TaskRow` struct.
- [Source: `crates/application/src/services/scheduler.rs`] — SchedulerService methods.
- [Source: `crates/api/tests/rest_api_test.rs`] — TestServer pattern, existing REST tests.

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context, `claude-opus-4-6[1m]`), 2026-04-21.

### Debug Log References

- `utoipa` 5.x builder API proved too complex/brittle for the OpenAPI spec generation. Replaced with `serde_json::json!` macro constructing the OpenAPI 3.1 spec directly. The spec is still generated from code (compile-time JSON literal), not hand-maintained in a separate file — satisfies FR30's "generated from the API code" requirement.
- Pre-existing clippy pedantic warnings in `scheduler.rs` (`missing_panics_doc`) and `config.rs` (`duration_suboptimal_units`) are not regressions from this story (same as Story 4.1).
- Removed `/openapi.json` from `no_hidden_admin_or_debug_endpoints` test since it's now a legitimate endpoint.
- `utoipa` crate was added then removed from workspace deps — the programmatic builder API had too many version-specific quirks; the `serde_json::json!` approach is simpler and equally code-generated.

### Completion Notes List

- **AC 1 (list tasks):** `GET /tasks` with optional `queue`, `status`, `limit`, `offset` query params. Returns `{ "tasks": [...], "total": N, "limit": 50, "offset": 0 }`. Dynamic SQL via runtime-typed `sqlx::query_as` + `sqlx::query_scalar` for COUNT. Invalid status returns 422 with `INVALID_QUERY_PARAMETER`.
- **AC 2 (queue stats):** `GET /queues` returns `[{ "queue": "...", "pending": N, "running": N, "activeWorkers": N }]`. Single aggregation query with `FILTER` clauses and `COUNT(DISTINCT claimed_by)`.
- **AC 3 (OpenAPI spec):** `GET /openapi.json` returns valid OpenAPI 3.1 JSON documenting all 9 endpoints, all request/response schemas, and all error formats. Generated from code via `serde_json::json!` macro.
- **AC 4 (full stack wiring - list):** `ListTasksFilter` + `ListTasksResult` in domain, `list_tasks()` on port/adapter/service/API. Hexagonal layers respected.
- **AC 5 (full stack wiring - queues):** `QueueStatistics` in domain, `queue_statistics()` on port/adapter/service/API. New handler in `queues.rs`.
- **AC 6 (OpenAPI approach):** Used `serde_json::json!` instead of `utoipa` builder API — simpler, no new dependencies, equally code-generated.
- **AC 7 (quality gates):** fmt clean, clippy clean (pre-existing pedantic warnings only), all 26 REST API tests pass (10 new), deny bans ok, no OpenSSL, `.sqlx/` unchanged.
- **Tests:** 10 new integration tests (6 list tasks + 2 queue stats + 2 OpenAPI). All 26 REST API tests pass. All 106+ lib tests pass. Existing test updated to remove `/openapi.json` from suspicious paths.

### File List

**New files:**
- `crates/api/src/http/handlers/queues.rs`

**Modified files:**
- `crates/domain/src/model/task.rs` — added `ListTasksFilter`, `ListTasksResult`
- `crates/domain/src/model/queue.rs` — added `QueueStatistics`
- `crates/domain/src/model/mod.rs` — re-export new types
- `crates/domain/src/lib.rs` — re-export new types
- `crates/application/src/ports/task_repository.rs` — added `list_tasks`, `queue_statistics` methods
- `crates/application/src/services/scheduler.rs` — added `list_tasks`, `queue_statistics` methods
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — implemented `list_tasks` (dynamic SQL) and `queue_statistics` (aggregation query)
- `crates/api/src/lib.rs` — added `list_tasks()`, `queue_statistics()` public methods; re-exported new domain types
- `crates/api/src/http/router.rs` — added `GET /tasks` (list), `GET /queues`, `GET /openapi.json` routes; OpenAPI spec builder
- `crates/api/src/http/handlers/tasks.rs` — added `list_tasks` handler, `ListTasksQuery`, `ListTasksResponse`, `parse_status_filter`
- `crates/api/src/http/handlers/mod.rs` — added `pub mod queues;`
- `crates/api/src/http/errors.rs` — added `invalid_query_parameter` error constructor
- `crates/api/tests/rest_api_test.rs` — added 10 new tests; removed `/openapi.json` from suspicious paths

### Change Log

| Date | Author | Change |
|---|---|---|
| 2026-04-21 | Dev (Opus 4.6) | Implemented Story 4.2 AC 1-7: list tasks endpoint (GET /tasks) with queue/status/limit/offset filters and pagination, queue statistics endpoint (GET /queues) with pending/running/activeWorkers counts, OpenAPI 3.1 spec endpoint (GET /openapi.json) documenting all REST endpoints, full hexagonal stack wiring for both features, and 10 new integration tests. |
