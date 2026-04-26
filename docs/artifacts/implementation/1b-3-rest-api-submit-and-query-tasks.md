# Story 1B.3: REST API — Submit & Query Tasks

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an external service,
I want to submit tasks and query their status via a REST API,
So that I can integrate iron-defer into non-Rust systems and workflows.

## Acceptance Criteria

1. **`POST /tasks` creates a task and returns HTTP 201:**
   - Request body: `{ "queue": "default", "kind": "EmailNotification", "payload": { "to": "user@example.com" } }`
   - Response: HTTP 201 with full task record in **camelCase** JSON: `{ "id": "uuid", "queue": "default", "kind": "EmailNotification", "status": "pending", "priority": 0, "attempts": 0, "scheduledAt": "...", "createdAt": "..." }`
   - `queue` defaults to `"default"` if omitted (Architecture line 1184).
   - `priority` defaults to 0, `maxAttempts` defaults to 3.
   - Optional fields: `scheduledAt` (ISO 8601 UTC), `priority` (i16), `maxAttempts` (i32).

2. **`POST /tasks` with invalid or missing fields returns HTTP 422:**
   - If `kind` is omitted, `payload` is not valid JSON, or queue name fails validation.
   - Error body: `{ "error": { "code": "INVALID_PAYLOAD", "message": "..." } }`

3. **`POST /tasks` with unknown `kind` returns HTTP 422:**
   - If no handler is registered for the given `kind` string, the request is rejected at the API boundary (fail-fast, consistent with `IronDefer::enqueue`'s existing registry check).
   - Error body: `{ "error": { "code": "INVALID_PAYLOAD", "message": "no handler registered for kind ..." } }`

4. **`GET /tasks/{id}` returns the task record:**
   - HTTP 200 with full task record in camelCase JSON.
   - If task not found: HTTP 404 with `{ "error": { "code": "TASK_NOT_FOUND", "message": "..." } }`

5. **HTTP server configuration:**
   - Request body size limited to **1 MiB** via axum `DefaultBodyLimit` (Architecture D4.2).
   - JSON field naming uses **camelCase** (ADR-0006).
   - Date/time fields use **ISO 8601 UTC** format.
   - Error codes are **SCREAMING_SNAKE_CASE** (Architecture line 643).

6. **`IronDefer` gains a `serve()` method** that starts the axum HTTP server:
   - Signature: `pub async fn serve(&self, bind: &str, token: CancellationToken) -> Result<(), TaskError>`
   - Binds to the given address (e.g. `"0.0.0.0:3000"`).
   - Wires graceful shutdown via `axum::serve(...).with_graceful_shutdown(token.cancelled())`.
   - The method blocks until the token fires and the server drains.

7. **`IronDefer` gains a `enqueue_raw()` method** for runtime-typed task submission:
   - Signature: `pub async fn enqueue_raw(&self, queue: &str, kind: &str, payload: Value, scheduled_at: Option<DateTime<Utc>>, priority: Option<i16>, max_attempts: Option<i32>) -> Result<TaskRecord, TaskError>`
   - Validates queue name, checks registry for handler matching `kind`, then delegates to `SchedulerService`.
   - This is the bridge between the REST API (runtime strings) and the typed library API.
   - `SchedulerService::enqueue` must be extended to accept `priority` and `max_attempts` overrides.

8. **Integration tests** in `crates/api/tests/rest_api_test.rs` using testcontainers:
   - **`post_task_returns_201_with_camel_case_body`** — POST a task, verify HTTP 201 + camelCase JSON fields.
   - **`post_task_with_scheduled_at_and_priority`** — POST with optional fields, verify they are stored.
   - **`post_task_missing_kind_returns_422`** — POST without `kind`, verify HTTP 422 + error body.
   - **`post_task_unknown_kind_returns_422`** — POST with unregistered kind, verify HTTP 422.
   - **`get_task_returns_200`** — POST then GET, verify round-trip.
   - **`get_nonexistent_task_returns_404`** — GET a random UUID, verify HTTP 404 + error body.
   - **`post_task_body_limit_enforced`** (**LOAD-BEARING TEST**) — POST a body exceeding 1 MiB, verify rejection.

9. **`#[instrument]` spans on all new public async methods** per Architecture lines 692-702:
   - `IronDefer::serve`: `#[instrument(skip(self, token), fields(bind = %bind), err)]`
   - `IronDefer::enqueue_raw`: `#[instrument(skip(self, payload), fields(queue = %queue, kind = %kind), err)]`
   - Handler functions: `#[instrument(skip_all, fields(method = "POST", path = "/tasks"), err)]` (or equivalent).
   - Payload is NEVER in fields (FR38).

10. **Quality gates pass:**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace` — all existing tests pass + new REST tests.
    - `cargo deny check bans` — `bans ok`

## Tasks / Subtasks

- [x] **Task 1: Create HTTP module structure** (AC 5, 6)
  - [x] Create `crates/api/src/http/mod.rs` with `pub mod router; pub mod handlers; pub mod errors;`
  - [x] Create `crates/api/src/http/router.rs` — axum `Router` factory with `DefaultBodyLimit` (1 MiB) middleware.
  - [x] Create `crates/api/src/http/errors.rs` — `AppError` struct implementing `axum::response::IntoResponse`. Maps `TaskError` to HTTP status codes + JSON error body.
  - [x] Create `crates/api/src/http/handlers/mod.rs` with `pub mod tasks;`
  - [x] Add `axum` dependency to `crates/api/Cargo.toml` (already in workspace as `axum = "0.8"`). Also add `tower-http` for `DefaultBodyLimit` if needed, or use axum's built-in.
  - [x] Add `pub mod http;` to `crates/api/src/lib.rs`.
  - [x] Run `cargo check -p iron-defer`.

- [x] **Task 2: Implement request/response DTOs** (AC 1, 5)
  - [x] Define `CreateTaskRequest` in handlers/tasks.rs: `{ queue: Option<String>, kind: String, payload: Value, scheduledAt: Option<DateTime<Utc>>, priority: Option<i16>, maxAttempts: Option<i32> }` with `#[serde(rename_all = "camelCase")]`.
  - [x] Define `TaskResponse` DTO with `#[serde(rename_all = "camelCase")]` that maps from `TaskRecord`. Include: `id`, `queue`, `kind`, `payload`, `status`, `priority`, `attempts`, `maxAttempts`, `lastError`, `scheduledAt`, `claimedBy`, `claimedUntil`, `createdAt`, `updatedAt`.
  - [x] Define `ErrorResponse { error: ErrorDetail }` and `ErrorDetail { code: String, message: String }` with `#[serde(rename_all = "camelCase")]`.
  - [x] Implement `From<TaskRecord> for TaskResponse`.

- [x] **Task 3: Implement `AppError` and error mapping** (AC 2, 4, 5)
  - [x] `AppError` wraps `TaskError` and adds a `not_found(msg)` constructor for 404.
  - [x] Implement `IntoResponse` for `AppError`:
    - `TaskError::InvalidPayload` → 422 + `INVALID_PAYLOAD`
    - `TaskError::AlreadyClaimed` → 409 + `TASK_ALREADY_CLAIMED`
    - `TaskError::Storage` → 500 + `INTERNAL_ERROR`
    - Custom not-found → 404 + `TASK_NOT_FOUND`
  - [x] Ensure error response body always matches `{ "error": { "code": "...", "message": "..." } }`.

- [x] **Task 4: Implement POST /tasks handler** (AC 1, 2, 3)
  - [x] Handler function: `async fn create_task(State(engine): State<Arc<IronDefer>>, Json(body): Json<CreateTaskRequest>) -> Result<(StatusCode, Json<TaskResponse>), AppError>`
  - [x] Validate `kind` is non-empty. If missing or empty → return 422.
  - [x] Call `engine.enqueue_raw(...)` with fields from request body.
  - [x] Return `StatusCode::CREATED` (201) with `Json(TaskResponse::from(record))`.

- [x] **Task 5: Implement GET /tasks/{id} handler** (AC 4)
  - [x] Handler function: `async fn get_task(State(engine): State<Arc<IronDefer>>, Path(id): Path<Uuid>) -> Result<Json<TaskResponse>, AppError>`
  - [x] Parse UUID from path. Call `engine.find(TaskId::from_uuid(id))`.
  - [x] If `Some(record)` → return 200 + `Json(TaskResponse::from(record))`.
  - [x] If `None` → return `AppError::not_found(format!("task {id} not found"))`.

- [x] **Task 6: Wire router and implement `IronDefer::serve()`** (AC 6)
  - [x] In `router.rs`: build `Router::new().route("/tasks", post(create_task)).route("/tasks/{id}", get(get_task)).layer(DefaultBodyLimit::max(1_048_576)).with_state(Arc::new(engine_ref))`.
  - [x] In `crates/api/src/lib.rs`: implement `serve(&self, bind: &str, token: CancellationToken)`. Construct router, bind TCP listener, serve with graceful shutdown.
  - [x] `serve()` must NOT consume `self` — it borrows the engine. Use `Arc<IronDefer>` as axum state by cloning within.

- [x] **Task 7: Implement `enqueue_raw()` and extend `SchedulerService`** (AC 7)
  - [x] Add `enqueue_raw()` to `IronDefer` that takes runtime `kind: &str`, `payload: Value`, optional `priority`, optional `max_attempts`.
  - [x] Add registry check: `self.registry.get(kind).is_none()` → return `InvalidPayload`.
  - [x] Extend `SchedulerService::enqueue` to accept optional `priority: Option<i16>` and `max_attempts: Option<i32>` parameters (use current defaults when None).
  - [x] Validate queue name, then delegate to `SchedulerService`.

- [x] **Task 8: Integration tests** (AC 8)
  - [x] Create `crates/api/tests/rest_api_test.rs` with shared testcontainers setup.
  - [x] Create a test helper that builds an `IronDefer` engine with a test task type registered, then starts the HTTP server on a random port.
  - [x] Implement all 7 test cases listed in AC 8.
  - [x] Use `reqwest` (already in workspace deps) as the HTTP client in tests.
  - [x] LOAD-BEARING TEST: body limit test must actually send >1 MiB payload.

- [x] **Task 9: Quality gates** (AC 10)
  - [x] `cargo fmt --check`
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
  - [x] `SQLX_OFFLINE=true cargo test --workspace`
  - [x] `cargo deny check bans`
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` returns empty.

## Dev Notes

### Architecture Compliance

- **D4.2 Request Body Limit:** 1 MiB via `DefaultBodyLimit` (axum built-in or tower-http). Architecture line 414.
- **ADR-0006 camelCase:** All REST request/response structs use `#[serde(rename_all = "camelCase")]`. Architecture line 765. Domain types (`TaskRecord`) use Rust naming; the DTO layer converts.
- **C1 Graceful Shutdown:** `axum::serve(...).with_graceful_shutdown(token.cancelled())` is REQUIRED. Architecture lines 1100-1107. Without it, the HTTP server keeps accepting connections during shutdown.
- **D4.1 No Authentication:** MVP has no auth. Architecture lines 407-412. Documented compliance gap.
- **Error response shape:** Always `{ "error": { "code": "...", "message": "..." } }`. Codes are `SCREAMING_SNAKE_CASE`. Architecture lines 628-645.
- **No `/v1/` prefix:** MVP routes are unprefixed. Architecture line 551.
- **HTTP module structure:** Architecture lines 900-905 specifies `api/src/http/` with `router.rs`, `handlers/tasks.rs`, `handlers/health.rs`, `errors.rs`. This story creates `tasks.rs` and `errors.rs`; `health.rs` is deferred to Story 4.1.

### Critical Design Decision: `enqueue_raw()` for REST API

The existing `IronDefer::enqueue<T: Task>()` requires a compile-time concrete type `T`. The REST API receives `kind` as a runtime string and `payload` as raw JSON — there is no concrete `T` to monomorphize over.

**Solution:** Add `IronDefer::enqueue_raw(queue, kind, payload, ...)` that:
1. Validates `kind` against the registry (same fail-fast as `enqueue<T>`)
2. Validates queue name via `QueueName::try_from`
3. Delegates to an extended `SchedulerService::enqueue` that accepts runtime values

`SchedulerService::enqueue` currently takes `kind: &'static str`. For `enqueue_raw`, we need to accept `kind: &str` (or `String`). The simplest approach: add a new `SchedulerService::enqueue_raw(queue, kind: &str, payload, scheduled_at, priority, max_attempts)` method that takes owned/borrowed strings instead of `&'static str`. This avoids modifying the existing typed enqueue path.

### Previous Story Intelligence (from Stories 1B.1 and 1B.2)

**Code patterns established that MUST be followed:**
- `#[instrument(skip(self), fields(...), err)]` on every public async method. Payload NEVER in fields.
- No `unwrap()` / `expect()` / `panic!()` in `src/` outside `#[cfg(test)]`. Map all errors to `TaskError`.
- Error source chains preserved — never discard context.
- `TaskRegistry::new()` constructed ONLY in `crates/api/src/lib.rs` (and `#[cfg(test)]`).
- Builder never spawns a Tokio runtime.
- Integration tests use shared `OnceCell<Option<TestDb>>` testcontainers pattern.
- Load-bearing tests verify via raw SQL, not just API return values.

**Key types and locations (verified current):**
- `TaskRecord` — `crates/domain/src/model/task.rs:71-86` — 14 fields, `#[non_exhaustive]`, derives `Serialize, Deserialize`
- `TaskId` — `crates/domain/src/model/task.rs:14-48` — UUID wrapper, `from_uuid()` and `as_uuid()` accessors, `Display` impl
- `QueueName` — `crates/domain/src/model/queue.rs` — validated string, `TryFrom<&str>`, `as_str()`, `Display`
- `TaskStatus` — `crates/domain/src/model/task.rs:54-62` — `#[serde(rename_all = "snake_case")]` (Pending, Running, Completed, Failed, Cancelled)
- `TaskError` — `crates/domain/src/error.rs` — variants: `AlreadyClaimed`, `InvalidPayload`, `ExecutionFailed`, `Storage`
- `SchedulerService` — `crates/application/src/services/scheduler.rs:36-119` — `enqueue(queue, kind: &'static str, payload, scheduled_at)`
- `TaskRegistry` — `crates/application/src/registry.rs:60-128` — `get(kind: &str) -> Option<&Arc<dyn TaskHandler>>`
- `IronDefer` — `crates/api/src/lib.rs:138-306` — fields: `scheduler`, `registry`, `pool`, `worker_config`, `queue`
- `IronDeferBuilder` — `crates/api/src/lib.rs:339-461`

**Important: `TaskRecord` serde naming mismatch.** `TaskRecord` currently derives `Serialize` with no `rename_all` override — field names are Rust `snake_case` by default. `TaskStatus` uses `#[serde(rename_all = "snake_case")]`. The REST API needs `camelCase` output. **Do NOT modify `TaskRecord`'s serde attributes** — the snake_case naming is correct for database serialization. Instead, create a `TaskResponse` DTO that maps from `TaskRecord` with `#[serde(rename_all = "camelCase")]`.

### Dependencies

**Already in workspace `Cargo.toml`:**
- `axum = { version = "0.8" }` — NOT yet added to `crates/api/Cargo.toml`
- `reqwest = { version = "0.12", ... }` — for test HTTP client
- `serde`, `serde_json`, `chrono` (with serde), `uuid` (with serde), `tokio`, `tracing` — all present

**May need to add:**
- `tower-http` — for `DefaultBodyLimit` layer (check if axum 0.8 provides this built-in; if not, add to workspace). Actually, `axum::extract::DefaultBodyLimit` is built into axum 0.8, so tower-http may not be needed for this specific middleware.
- `tower` — for `ServiceBuilder` if composing middleware layers. May not be needed if using axum's `.layer()` directly.

**Verify before coding:** Check `axum` 0.8 API for `DefaultBodyLimit` availability. In axum 0.8, `DefaultBodyLimit` is at `axum::extract::DefaultBodyLimit`. Use `.layer(DefaultBodyLimit::max(1_048_576))` on the router.

### Axum State Pattern

Axum handlers access shared state via `State<T>` extractor. Since `IronDefer` is not `Clone`, wrap in `Arc`:

```rust
let engine = Arc::new(engine);
let app = Router::new()
    .route("/tasks", post(create_task))
    .route("/tasks/{id}", get(get_task))
    .layer(DefaultBodyLimit::max(1_048_576))
    .with_state(engine);
```

Handler signature:
```rust
async fn create_task(
    State(engine): State<Arc<IronDefer>>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskResponse>), AppError> { ... }
```

### Test Server Pattern

For integration tests, start the server on a random port (`0.0.0.0:0`), extract the bound address, then use `reqwest` to send requests:

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let addr = listener.local_addr()?;
let token = CancellationToken::new();
// Spawn server in background
tokio::spawn(async move { axum::serve(listener, app).with_graceful_shutdown(token.cancelled()).await });
// Use reqwest against http://127.0.0.1:{port}/tasks
```

### Out of Scope for This Story

- **`GET /tasks` (list)** — deferred to Story 4.2 (with filters, pagination).
- **`DELETE /tasks/{id}` (cancel)** — deferred to Story 4.1.
- **Health check endpoints** (`GET /health/live`, `GET /health/ready`) — Story 4.1.
- **Authentication** — deferred to Growth phase (Architecture D4.1).
- **API versioning** (`/v1/` prefix) — deferred to Growth phase.
- **CLI submit/inspect** — Epic 4.
- **CORS middleware** — not required for MVP (internal network deployment).
- **`main.rs` wiring** (standalone binary) — Epic 4.

### Project Structure Notes

- **New files:**
  - `crates/api/src/http/mod.rs`
  - `crates/api/src/http/router.rs`
  - `crates/api/src/http/errors.rs`
  - `crates/api/src/http/handlers/mod.rs`
  - `crates/api/src/http/handlers/tasks.rs`
  - `crates/api/tests/rest_api_test.rs`
- **Modified files:**
  - `crates/api/src/lib.rs` — add `pub mod http;`, `serve()`, `enqueue_raw()`
  - `crates/api/Cargo.toml` — add `axum` dependency
  - `crates/application/src/services/scheduler.rs` — extend `enqueue` or add `enqueue_raw` variant
  - `Cargo.toml` (workspace) — may need `tower-http` if required

### References

- [Source: `docs/artifacts/planning/architecture.md` §D4.2 line 414] — Request body size limit (1 MiB)
- [Source: `docs/artifacts/planning/architecture.md` lines 548-551] — API endpoint naming conventions
- [Source: `docs/artifacts/planning/architecture.md` lines 628-656] — Response shape, error codes, HTTP status mapping
- [Source: `docs/artifacts/planning/architecture.md` lines 1184-1192] — POST /tasks single endpoint (C5)
- [Source: `docs/artifacts/planning/architecture.md` lines 1100-1107] — Axum graceful shutdown (C1)
- [Source: `docs/artifacts/planning/architecture.md` lines 900-920] — API crate file structure
- [Source: `docs/artifacts/planning/architecture.md` lines 961-964] — Handler → SchedulerService mapping
- [Source: `docs/artifacts/planning/architecture.md` lines 407-412] — No authentication in MVP (D4.1)
- [Source: `docs/artifacts/planning/architecture.md` line 765] — camelCase JSON (ADR-0006)
- [Source: `docs/artifacts/planning/architecture.md` lines 692-702] — `#[instrument]` conventions
- [Source: `docs/artifacts/planning/epics.md`] — Epic 1B, Story 1B.3 acceptance criteria
- [Source: `docs/artifacts/implementation/1b-2-worker-pool-and-execution-loop.md`] — Previous story patterns
- [Source: `crates/application/src/services/scheduler.rs`] — SchedulerService current API
- [Source: `crates/application/src/registry.rs`] — TaskRegistry::get(&str)
- [Source: `crates/api/src/lib.rs`] — IronDefer builder, enqueue, find, list

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None — clean implementation with no blockers or failures.

### Completion Notes List

- **Task 1:** Created `crates/api/src/http/` module structure: `mod.rs`, `router.rs`, `errors.rs`, `handlers/mod.rs`, `handlers/tasks.rs`. Added `axum` and `uuid` to `crates/api/Cargo.toml` dependencies. Added `reqwest` with `json` feature to dev-dependencies.
- **Task 2:** Defined `CreateTaskRequest` (camelCase, optional queue/scheduledAt/priority/maxAttempts) and `TaskResponse` (camelCase, maps from `TaskRecord`) DTOs. Defined `ErrorResponse` + `ErrorDetail` for structured error bodies.
- **Task 3:** Implemented `AppError` with `Display`, `IntoResponse`, and `From<TaskError>`. Maps: `InvalidPayload` → 422, `AlreadyClaimed` → 409, `Storage`/`ExecutionFailed` → 500. Added `not_found()` constructor for 404.
- **Task 4:** Implemented `POST /tasks` handler: extracts `CreateTaskRequest`, defaults queue to `"default"`, calls `engine.enqueue_raw()`, returns 201 + `TaskResponse`.
- **Task 5:** Implemented `GET /tasks/{id}` handler: parses UUID from path, calls `engine.find()`, returns 200 + `TaskResponse` or 404 `AppError`.
- **Task 6:** Built axum `Router` with `DefaultBodyLimit::max(1_048_576)` (1 MiB). Implemented `IronDefer::serve()` that binds TCP listener, serves router with graceful shutdown via `token.cancelled_owned()`.
- **Task 7:** Added `IronDefer::enqueue_raw()` for runtime-typed task submission (REST bridge). Extended `SchedulerService` with `enqueue_raw()` that accepts `&str` kind + optional priority/max_attempts overrides.
- **Task 8:** All 7 integration tests implemented: `post_task_returns_201_with_camel_case_body`, `post_task_with_scheduled_at_and_priority`, `post_task_missing_kind_returns_422`, `post_task_unknown_kind_returns_422`, `get_task_returns_200`, `get_nonexistent_task_returns_404`, `post_task_body_limit_enforced` (LOAD-BEARING — sends >1 MiB payload, verifies 413).
- **Task 9:** All quality gates pass: `cargo fmt --check`, `cargo clippy --pedantic`, 96 tests (90 existing + 6 new), `cargo deny check bans`, no openssl/native-tls in production graph.

### File List

- `crates/api/src/http/mod.rs` — new (HTTP module root)
- `crates/api/src/http/router.rs` — new (axum Router factory with 1 MiB body limit)
- `crates/api/src/http/errors.rs` — new (AppError + IntoResponse + ErrorResponse)
- `crates/api/src/http/handlers/mod.rs` — new (handlers module root)
- `crates/api/src/http/handlers/tasks.rs` — new (POST/GET handlers + request/response DTOs)
- `crates/api/src/lib.rs` — modified (add `pub mod http`, `serve()`, `enqueue_raw()`)
- `crates/api/Cargo.toml` — modified (add axum, uuid, reqwest dependencies)
- `crates/application/src/services/scheduler.rs` — modified (add `enqueue_raw()` method)
- `crates/api/tests/rest_api_test.rs` — new (7 integration tests)

### Review Findings

- [x] [Review][Patch] Storage errors leak DB internals in 500 responses — fixed: 500 responses now return generic "internal server error" message; real error logged server-side via `tracing::error!`. [crates/api/src/http/errors.rs]
- [x] [Review][Patch] `max_attempts` accepts 0 and negative values via REST API — fixed: added `max_attempts >= 1` validation in `enqueue_raw()`. [crates/api/src/lib.rs]
- [x] [Review][Defer] Axum extractor rejections (Json, Path) return unstructured error, not JSON envelope — axum's default `JsonRejection` and `PathRejection` return plain text, not our `{"error":{...}}` format. Requires custom extractors. Deferred to a future story when more endpoints land. [crates/api/src/http/handlers/tasks.rs] — deferred, broader concern

### Change Log

- 2026-04-11: Implemented Story 1B.3 — REST API with POST /tasks (201) and GET /tasks/{id} (200/404). axum-based HTTP server with 1 MiB body limit, camelCase JSON responses, structured error bodies. Added enqueue_raw() bridge for runtime-typed task submission. 7 integration tests. All 96 tests pass, all quality gates green.
