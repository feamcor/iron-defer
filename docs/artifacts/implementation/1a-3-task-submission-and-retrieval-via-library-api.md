# Story 1A.3: Task Submission & Retrieval via Library API

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a Rust developer,
I want to submit tasks and retrieve their status through an embedded library API,
so that I can integrate durable task execution into my existing Tokio application.

## Acceptance Criteria

1. **`Task` trait gains `Serialize + DeserializeOwned` supertrait bounds** matching architecture §D7.2.
   - `crates/domain/src/model/task.rs`: change `pub trait Task: Send + Sync + 'static` to `pub trait Task: Send + Sync + Serialize + DeserializeOwned + 'static`.
   - The bounds are required because `TaskHandlerAdapter` (AC 5) deserializes `T` from a `serde_json::Value` payload.
   - Update the inline `DummyTask` test fixture so it derives `Serialize, Deserialize` and remains compilable.
   - This resolves a deferred-work item from Story 1A.1.

2. **`TaskHandler` object-safe trait + `TaskRegistry`** live in `crates/application/src/registry.rs`.
   - `pub trait TaskHandler: Send + Sync` with two methods:
     - `fn kind(&self) -> &'static str;`
     - `fn execute<'a>(&'a self, payload: &'a serde_json::Value, ctx: &'a TaskContext) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>>;`
   - **Manual `Pin<Box<dyn Future>>`** — NOT `#[async_trait]`. Architecture line 1149 specifies this shape because TaskHandler is the per-task hot-path and we want explicit boxing control. (`async-trait` can be added later if profiling shows the macro is fine.)
   - `pub struct TaskRegistry { handlers: HashMap<&'static str, Arc<dyn TaskHandler>> }` with:
     - `pub fn new() -> Self` (also derive/implement `Default`)
     - `pub fn register(&mut self, handler: Arc<dyn TaskHandler>)` — uses `handler.kind()` as the key
     - `pub fn get(&self, kind: &str) -> Option<&Arc<dyn TaskHandler>>` — returns `None` for unregistered kinds (Epic 1B's worker pool turns the `None` into a panic with descriptive message; **the registry itself does not panic on lookup**, panic happens at the dispatch site)
   - The registry is constructed in the api crate only — architecture lines 778, 665–667 forbid construction in `application` or `infrastructure`. The application crate exposes the type so `Arc<TaskRegistry>` can be injected as a dependency, but it never instantiates one.

3. **`SchedulerService`** in `crates/application/src/services/scheduler.rs` provides typed enqueue / find / list methods backed by the `TaskRepository` port.
   - `pub struct SchedulerService { repo: Arc<dyn TaskRepository> }`
   - `pub fn new(repo: Arc<dyn TaskRepository>) -> Self`
   - `pub async fn enqueue(&self, queue: &QueueName, kind: &'static str, payload: serde_json::Value, scheduled_at: Option<DateTime<Utc>>) -> Result<TaskRecord, TaskError>`
     - Constructs a fresh `TaskRecord` via `TaskRecord::new(...)`: `TaskId::new()`, the supplied queue, kind, payload, `TaskStatus::Pending`, `priority = 0`, `attempts = 0`, `max_attempts = 3` (constants for now — exposed in a later story), `last_error = None`, `scheduled_at = scheduled_at.unwrap_or_else(Utc::now)`, `claimed_by = None`, `claimed_until = None`, and placeholder `created_at = updated_at = Utc::now()` (the database `DEFAULT now()` overwrites them on insert per Story 1A.2).
     - Delegates to `self.repo.save(&record).await`.
     - Decorated with `#[instrument(skip(self, payload), fields(queue = %queue, kind = %kind), err)]` — payload skipped for FR38 privacy.
   - `pub async fn find(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError>` — delegates to `self.repo.find_by_id(id).await`.
   - `pub async fn list(&self, queue: &QueueName) -> Result<Vec<TaskRecord>, TaskError>` — delegates to `self.repo.list_by_queue(queue).await`.
   - All public async methods carry `#[instrument(skip(self), ...)]` per architecture lines 692–702.
   - Unit-tested with **`mockall`** mocks of `TaskRepository` (no real DB needed) — see AC 7.

4. **`Task` port mocking via `mockall`** — `crates/application/src/ports/task_repository.rs` decorated with `#[cfg_attr(test, mockall::automock)]` (or unconditional `#[mockall::automock]` if it must be visible to downstream test code). Architecture C6 requires `mockall` for application port traits.
   - Add `mockall = "0.13"` to `[workspace.dependencies]` in the workspace root `Cargo.toml`.
   - Add `mockall = { workspace = true }` to `[dev-dependencies]` in `crates/application/Cargo.toml`.
   - The same `#[automock]` annotation goes on `TaskExecutor` for symmetry, even though no consumer mocks it yet.

5. **`TaskHandlerAdapter<T: Task>`** lives in `crates/api/src/lib.rs` (the SOLE crate where logic in `lib.rs` is permitted, architecture lines 562–565).
   - `struct TaskHandlerAdapter<T: Task>(PhantomData<T>);`
   - `impl<T: Task> TaskHandler for TaskHandlerAdapter<T>`:
     - `fn kind(&self) -> &'static str { T::KIND }`
     - `fn execute<'a>(...) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>>` deserializes `T` from `payload.clone()` via `serde_json::from_value`, mapping serde errors to `TaskError::InvalidPayload { reason: e.to_string() }`, then calls `task.execute(ctx).await`.
   - `TaskHandlerAdapter` itself is `pub(crate)` — never crosses the public API boundary. Only the `register::<T>()` builder method creates one.

6. **`IronDefer` + `IronDeferBuilder`** live in `crates/api/src/lib.rs` and form the public library entry point per architecture §D7.1 (lines 466–491).

   **`IronDefer` engine** holds:
   - `scheduler: SchedulerService`
   - `registry: Arc<TaskRegistry>`
   - `pool: PgPool` (kept around so `IronDefer::pool() -> &PgPool` can return it for callers who want to share the pool)
   - Public methods (this story):
     - `pub fn builder() -> IronDeferBuilder`
     - `pub async fn enqueue<T: Task>(&self, queue: &str, task: T) -> Result<TaskRecord, TaskError>` — serializes `task` to `serde_json::Value` via `serde_json::to_value`, validates the queue name with `QueueName::try_from` (mapping `ValidationError` to `TaskError::InvalidPayload { reason }`), then delegates to `scheduler.enqueue(...)` with `kind = T::KIND` and no explicit `scheduled_at`.
     - `pub async fn enqueue_at<T: Task>(&self, queue: &str, task: T, scheduled_at: DateTime<Utc>) -> Result<TaskRecord, TaskError>` — same as `enqueue` but passes `Some(scheduled_at)` through.
     - `pub async fn find(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError>`
     - `pub async fn list(&self, queue: &str) -> Result<Vec<TaskRecord>, TaskError>` — validates queue name, delegates to scheduler.
     - `pub fn pool(&self) -> &PgPool`
     - `pub fn registry(&self) -> &Arc<TaskRegistry>`
     - `pub const fn migrator() -> &'static sqlx::migrate::Migrator { &iron_defer_infrastructure::MIGRATOR }` (architecture line 1140)
   - **No `start()` method in this story** — worker pool / sweeper are Epic 1B. Document with a `// TODO(Epic 1B)` comment.

   **`IronDeferBuilder`** holds:
   - `pool: Option<PgPool>`
   - `registry: TaskRegistry` (built up in place, wrapped in `Arc` at `build()` time)
   - `skip_migrations: bool` (defaults `false`)
   - Methods:
     - `pub fn pool(mut self, pool: PgPool) -> Self` — accepts the caller-provided `PgPool` (architecture line 938: this is the SOLE infrastructure type that may cross the public API boundary).
     - `pub fn register<T: Task>(mut self) -> Self` — constructs `Arc::new(TaskHandlerAdapter::<T>(PhantomData))` and inserts it into the registry under `T::KIND`.
     - `pub fn skip_migrations(mut self, skip: bool) -> Self`
     - `pub async fn build(self) -> Result<IronDefer, TaskError>` — pre-flight checks: `pool` must be set (otherwise `Err(TaskError::Storage { source: "PgPool not provided to IronDeferBuilder" boxed })`); if `!skip_migrations`, runs `iron_defer_infrastructure::MIGRATOR.run(&pool).await` and translates errors to `TaskError::Storage`. Constructs `PostgresTaskRepository` from the pool, wraps in `Arc<dyn TaskRepository>`, builds `SchedulerService::new(repo)`, wraps `registry` in `Arc`, returns the `IronDefer`.
   - **The builder NEVER spawns a Tokio runtime** (architecture line 776). The `build().await` runs migrations on the caller's runtime.

7. **Public API re-exports** in `crates/api/src/lib.rs`:
   - `pub use iron_defer_domain::{Task, TaskContext, TaskError, TaskId, TaskRecord, TaskStatus, QueueName};`
   - `pub use iron_defer_application::{TaskHandler, TaskRegistry};`
   - The crate re-exports DO NOT expose any infrastructure type other than the `&'static Migrator` returned by `IronDefer::migrator()` — architecture lines 681–683.
   - **Forbidden re-exports:** `sqlx::Error`, `axum::*`, `reqwest::*`, `iron_defer_infrastructure::PostgresTaskRepository`, `iron_defer_infrastructure::PostgresAdapterError`. None of these may appear in the public API surface.

8. **`crates/api/Cargo.toml` updated** with the deps actually needed by the builder:
   - Add `sqlx = { workspace = true }` so the public API can accept `PgPool` and call `MIGRATOR.run(...)`.
   - Add `chrono = { workspace = true }` and `serde_json = { workspace = true }` for the enqueue surface.
   - Add `async-trait = { workspace = true }` is **not** needed (TaskHandlerAdapter does manual `Pin<Box<...>>` per architecture line 1149).
   - Add `tokio = { workspace = true }` for `tokio::main` in the binary (already implied via tracing-subscriber but make explicit).

9. **Integration tests in `crates/api/tests/`** mirror the 1A.2 testcontainers pattern but live one crate up.
   - `crates/api/tests/common/mod.rs` — copy the `OnceCell<Option<TestDb>>` shared-pool pattern from `crates/infrastructure/tests/common/mod.rs`. Same skip-on-no-Docker behavior. Migrations run via `iron_defer::IronDefer::migrator().run(&pool).await` (i.e. the public API path, not the infrastructure crate's `MIGRATOR` directly — this exercises AC 6's migrator surface).
   - `crates/api/tests/integration_test.rs` covers:
     - **`builder_requires_pool`** — call `IronDefer::builder().build().await`, expect `Err(TaskError::Storage)` with message containing "PgPool not provided".
     - **`builder_runs_migrations_by_default`** — fresh container, `IronDefer::builder().pool(pool.clone()).build().await` should succeed and the `tasks` table should exist (verify with a raw `SELECT count(*) FROM tasks` query that returns `Ok(_)`).
     - **`builder_skip_migrations_does_not_run_migrator`** — fresh container with NO prior migration, `IronDefer::builder().pool(pool.clone()).skip_migrations(true).build().await` should succeed; a subsequent `SELECT count(*) FROM tasks` should error because the table does not exist.
     - **`enqueue_persists_task_with_kind_and_default_scheduled_at`** — register `EchoTask`, enqueue an instance, assert the returned `TaskRecord` has `kind == EchoTask::KIND`, `status == Pending`, `queue` matches input, payload round-trips (deserialize the returned `payload` back into `EchoTask` and assert equality), and `scheduled_at` is within 5 seconds of `Utc::now()`.
     - **`enqueue_at_respects_explicit_schedule`** — same setup, call `enqueue_at(..., now + 1 hour)`, assert returned `scheduled_at` matches (within 1ms tolerance).
     - **`find_returns_full_record`** — enqueue + `find(returned.id)`, assert `Some(record)` with all fields matching the saved record.
     - **`find_returns_none_for_missing_id`** — call `find(TaskId::new())`, expect `Ok(None)`.
     - **`list_returns_only_matching_queue`** — enqueue 3 tasks in `"payments"` and 2 in `"notifications"`, call `list("payments")` and `list("notifications")`, assert counts.
     - **`enqueue_rejects_invalid_queue_name`** — call `engine.enqueue("", task)` (empty string), expect `Err(TaskError::InvalidPayload)` with the underlying `ValidationError` message visible.
   - Each test scopes its writes with `unique_queue()` (helper in `common/mod.rs`) where it does not need a fixed queue name.
   - Integration tests use a sample `EchoTask` defined in the test module itself: `#[derive(Serialize, Deserialize, PartialEq, Debug)] struct EchoTask { message: String }` with `impl Task for EchoTask { const KIND: &'static str = "echo"; async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> { Ok(()) } }`.

10. **Application crate unit tests for `SchedulerService`** in `crates/application/src/services/scheduler.rs` use `mockall` to mock `TaskRepository`:
    - **`enqueue_calls_repo_save_with_constructed_record`** — mock expects `save()` called once, captures the `TaskRecord` argument, asserts `kind`, `queue`, `payload`, `status == Pending`, `attempts == 0`. Returns a synthetic `Ok(TaskRecord)`.
    - **`enqueue_uses_now_when_scheduled_at_omitted`** — mock captures the record, asserts `scheduled_at` is within 5 seconds of `Utc::now()`.
    - **`enqueue_passes_through_explicit_scheduled_at`** — mock captures the record, asserts `scheduled_at` exactly matches the input.
    - **`find_delegates_to_repo`** — mock expects `find_by_id(specific_id)` and returns a synthetic record.
    - **`list_delegates_to_repo`** — mock expects `list_by_queue(specific_queue_name)` and returns a fixed vec.

11. **Quality gates pass.**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace` — all unit tests + new SchedulerService mock tests + new api integration tests pass (integration tests skip cleanly when Docker is unavailable).
    - `cargo deny check bans` — still `bans ok`. The api crate now pulls `sqlx` directly (not just via infrastructure transitively), but the resolved feature set is unchanged. Re-verify with `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` returning empty.
    - `.sqlx/` cache: no new queries in this story (the api crate uses the infrastructure adapter, not raw sqlx macros), so no regeneration needed — but verify by running `SQLX_OFFLINE=true cargo check --workspace` clean.

## Tasks / Subtasks

- [x] **Task 1: Add `Serialize + DeserializeOwned` supertrait bounds to `Task`** (AC 1 — deferred from 1A.1)
  - [x] Edit `crates/domain/src/model/task.rs`: change `pub trait Task: Send + Sync + 'static` to `pub trait Task: Send + Sync + serde::Serialize + serde::de::DeserializeOwned + 'static`. Reference architecture §D7.2 lines 495–498 in a doc comment.
  - [x] Update the inline `DummyTask` test fixture: add `#[derive(serde::Serialize, serde::Deserialize)]`. The struct currently has zero fields (`struct DummyTask;`) so an empty serde derive is fine.
  - [x] Run `cargo check --workspace` and fix any other compile errors caused by the bound. (None expected — no production code currently constrains `T: Task` outside this trait definition.)
  - [x] Add a unit test in `task.rs`: `serde_json::from_value::<DummyTask>(serde_json::json!({}))` succeeds (smoke-tests the new bounds).

- [x] **Task 2: Add `mockall` workspace dep and annotate ports** (AC 4)
  - [x] Add `mockall = "0.13"` to `[workspace.dependencies]` in the workspace root `Cargo.toml`. Place it in the `# Dev` section next to `testcontainers`.
  - [x] Add `mockall = { workspace = true }` to `[dev-dependencies]` in `crates/application/Cargo.toml`.
  - [x] Annotate `TaskRepository` in `crates/application/src/ports/task_repository.rs` with `#[cfg_attr(test, mockall::automock)]` immediately above the `#[async_trait]` line. Do the same for `TaskExecutor` in `task_executor.rs`.
  - [x] Run `cargo test -p iron-defer-application` and confirm the mock types are generated (no test failures, just verifies compilation).

- [x] **Task 3: Implement `TaskHandler` + `TaskRegistry`** (AC 2)
  - [x] Create `crates/application/src/registry.rs` with the `TaskHandler` trait per architecture line 1149 — manual `Pin<Box<dyn Future + Send>>` return type, NOT `#[async_trait]`. Doc-comment why (per-task hot path, explicit boxing control).
  - [x] Define `pub struct TaskRegistry { handlers: HashMap<&'static str, Arc<dyn TaskHandler>> }` with `pub fn new() -> Self`, `impl Default`, `pub fn register(&mut self, handler: Arc<dyn TaskHandler>)`, and `pub fn get(&self, kind: &str) -> Option<&Arc<dyn TaskHandler>>`.
  - [x] Add `pub mod registry;` to `crates/application/src/lib.rs` and re-export `pub use crate::registry::{TaskHandler, TaskRegistry};`.
  - [x] Unit tests in `registry.rs`:
    - `new_registry_is_empty` — `TaskRegistry::new().get("anything").is_none()`
    - `register_then_get_returns_handler` — define a test-local `MockHandler` struct that implements `TaskHandler` returning `Ok(())`, register it, assert `get("mock")` is `Some`.
    - `register_overwrites_existing_kind` — register two handlers with the same `kind()`, assert the second wins (HashMap insert semantics).

- [x] **Task 4: Implement `SchedulerService`** (AC 3, 10)
  - [x] Create `crates/application/src/services/mod.rs` declaring `pub mod scheduler;` and re-exporting `pub use scheduler::SchedulerService;`.
  - [x] Add `pub mod services;` to `crates/application/src/lib.rs` and re-export `pub use crate::services::SchedulerService;`.
  - [x] Add `chrono` and `serde_json` as workspace deps to `crates/application/Cargo.toml` if not already present (check first; both are likely missing because 1A.1 only added bare minimum).
  - [x] Create `crates/application/src/services/scheduler.rs` with the `SchedulerService` struct, `new` constructor, and the four async methods from AC 3.
  - [x] Each method gets `#[instrument(skip(self, payload), fields(...), err)]` (payload-skipping is critical — FR38).
  - [x] **Unit tests with mockall** (AC 10):
    - Use `MockTaskRepository::new()` from the `#[automock]` derive
    - Set expectations with `expect_save()`, `expect_find_by_id()`, `expect_list_by_queue()`
    - Use `mockall::predicate::function` or `with(eq(...))` to assert call arguments
    - Each test constructs a `SchedulerService` from `Arc::new(mock) as Arc<dyn TaskRepository>`
  - [x] All 5 unit tests from AC 10.

- [x] **Task 5: Implement `TaskHandlerAdapter` and `IronDefer` engine in api crate** (AC 5, 6, 7, 8)
  - [x] Update `crates/api/Cargo.toml` per AC 8: add `sqlx`, `chrono`, `serde_json` workspace deps.
  - [x] Replace the placeholder `crates/api/src/lib.rs` with the real builder. The new file should contain:
    - Module-level `#![forbid(unsafe_code)]` (already present)
    - Doc comment confirming this is the SOLE crate where logic in `lib.rs` is permitted (preserve the existing note)
    - Public re-exports per AC 7
    - `struct TaskHandlerAdapter<T: Task>(PhantomData<T>);` — `pub(crate)`
    - `impl<T: Task> TaskHandler for TaskHandlerAdapter<T>` — manual `Pin<Box<dyn Future + Send>>` per architecture line 1149
    - `pub struct IronDefer { scheduler: SchedulerService, registry: Arc<TaskRegistry>, pool: PgPool }`
    - `pub struct IronDeferBuilder { pool: Option<PgPool>, registry: TaskRegistry, skip_migrations: bool }`
    - `impl Default for IronDeferBuilder` (constructs an empty registry, no pool, `skip_migrations = false`)
    - `IronDefer::builder()` returns `IronDeferBuilder::default()`
    - The builder methods + `build()` per AC 6
    - The engine's public API methods + `migrator()` accessor
  - [x] **Critical:** keep `run_placeholder()` until `crates/api/src/main.rs` is updated to use the new entry point. Then delete `run_placeholder()`. **This story does NOT delete `run_placeholder()`** — the binary's full wiring (CLI, HTTP server, signal handling) is Epic 4. Update `main.rs` only minimally so the binary still compiles after the placeholder is removed... actually leave `run_placeholder()` in place; the binary keeps its existing stub until Epic 4. Add a `#[allow(dead_code)]` if clippy complains.
  - [x] Verify the public API does not leak any forbidden types: `cargo doc -p iron-defer --no-deps` and visually scan the rendered surface (or `cargo check --workspace` and examine `iron_defer::*` re-exports).

- [x] **Task 6: Reconcile `TaskContext` shape** (touches AC 5)
  - [x] Architecture §D7.2 lines 500–504 specifies `TaskContext { task_id: TaskId, attempt: u32, queue: String }`. Story 1A.1 chose `task_id: TaskId, worker_id: WorkerId, attempt: i32` to align with `TaskRecord::attempts: i32` and to thread the executing worker through. The architecture's `worker_id`-less shape was written before the 1A.1 review hardening.
  - [x] **Decision for this story:** keep the 1A.1 shape (`worker_id` + `attempt: i32`) — Epic 1B's executor will need both. Add a doc comment to `TaskContext` explaining the deviation from architecture §D7.2 and crediting the rationale. Do NOT add `queue: String` yet — Epic 1B can decide whether to thread it through.
  - [x] No code changes outside the doc comment. This task exists so the next story author has a clear answer for "why does TaskContext look different from D7.2?".

- [x] **Task 7: Integration tests in `crates/api/tests/`** (AC 9)
  - [x] Add `[dev-dependencies]` to `crates/api/Cargo.toml`: `iron-defer-infrastructure` (path), `testcontainers`, `testcontainers-modules`, `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }`, `serde_json`, `serde = { workspace = true }`. The infrastructure crate is a dev-dep here so the test setup can spin up containers without it leaking into the public api graph.
  - [x] Create `crates/api/tests/common/mod.rs` mirroring the 1A.2 pattern: `OnceCell<Option<TestDb>>`, `pub async fn test_pool() -> Option<&'static PgPool>`, runtime-skip-on-no-Docker. Migrations run via `iron_defer::IronDefer::migrator().run(&pool)`. Helper `unique_queue() -> String`.
  - [x] Create `crates/api/tests/integration_test.rs` with all 9 test cases from AC 9. Use a local `EchoTask` struct (Serialize, Deserialize, derive PartialEq for assertions). Each test acquires the pool and constructs an `IronDefer` via `builder().pool(pool.clone()).register::<EchoTask>().build().await`.
  - [x] Tests that need to assert "table does not exist" use raw `sqlx::query("SELECT count(*) FROM tasks")` against the pool and assert the error matches the SQLSTATE for "undefined table" (`42P01`) — or just match on the error string containing "tasks" and "does not exist".

- [x] **Task 8: Wire `TaskRegistry` ownership rule** (AC 2)
  - [x] Confirm by code search: `TaskRegistry::new()` is called only in `crates/api/src/lib.rs` (via `IronDeferBuilder::default()`). It must NEVER appear in `crates/application/src/` outside the trait/struct definition itself, and NEVER in `crates/infrastructure/src/`.
  - [x] Add a `#[deny(...)]` is not feasible in Rust for "this constructor must only be called from one crate", so document the rule with a doc comment on `TaskRegistry::new` and add a Critical Convention bullet to the Dev Notes for future story authors.

- [x] **Task 9: Resolve idempotency / duplicate-id question** (AC 6 — deferred from 1A.2)
  - [x] Story 1A.2's deferred-work entry: "save() is INSERT-only with no upsert / no distinct duplicate-id error variant". Story 1A.3 owns the resolution.
  - [x] **Decision for this story:** `IronDefer::enqueue` ALWAYS generates a fresh `TaskId::new()` — there is no way for a caller to supply their own ID through the public API in this story. Therefore the duplicate-id case cannot be triggered by well-behaved callers via the library API surface. The PostgresTaskRepository `save()` method still maps `sqlx::Error::Database` (PK violation) to `TaskError::Storage` opaquely, but no production code path can hit it.
  - [x] **Caller-supplied idempotency keys** are deferred to a later story (likely Story 4.1 or a new "IdempotencyKey" story). Document this in the deferred-work file under a new heading.
  - [x] Update `docs/artifacts/implementation/deferred-work.md`: mark the "duplicate-id error variant" item as PARTIALLY RESOLVED — explicit resolution: enqueue API does not expose `TaskId` to callers, so the case cannot be reached. Caller-supplied ids deferred indefinitely.

- [x] **Task 10: Quality gates** (AC 11)
  - [x] `cargo fmt --check`
  - [x] `SQLX_OFFLINE=true cargo check --workspace`
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
  - [x] `SQLX_OFFLINE=true cargo test --workspace` — confirm 44 + new tests pass (5 new SchedulerService unit tests + 9 new api integration tests + ~3 new TaskRegistry unit tests + 1 new DummyTask serde smoke test ≈ +18, target ≈ 62 tests)
  - [x] `cargo deny check bans` — still `bans ok`
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` returns empty
  - [x] Update `docs/artifacts/implementation/deferred-work.md`: mark the "Task trait missing Serialize + DeserializeOwned supertrait bounds" item as RESOLVED in this story (was Epic 1B, now done in 1A.3).

### Review Findings

_Adversarial code review (2026-04-06): 3 layers — Blind Hunter, Edge Case Hunter, Acceptance Auditor._

**Decision needed (resolve before patches):**

- [x] [Review][Decision] **`IronDefer::enqueue` accepts unregistered kinds → orphan tasks** — `crates/api/src/lib.rs:184-219`. The enqueue path never consults `self.registry`. A caller can `register::<EmailTask>().build()` then `enqueue::<OtherTask>(...)` and durably persist a row that no worker can ever execute — the worker pool will panic at dispatch (per the architectural contract "missing registration ... panics"), turning a registration typo into a runtime crash long after the enqueue. Architecture line 778 bans constructing `TaskRegistry` outside the api crate, and AC 2 says missing registration "panics with a descriptive message — never silent task drop." But "panic at dispatch" is the worker-pool's responsibility, not the enqueue surface's. Two options: **(a) accept the deviation** — keep enqueue permissive, document that handler registration must happen before enqueue, fail-fast happens at worker dispatch (Epic 1B); **(b) tighten enqueue** — add an `if self.registry.get(T::KIND).is_none() { return Err(TaskError::InvalidPayload { reason: format!("no handler registered for kind {}", T::KIND) }); }` guard before persistence. Option (b) catches typos at enqueue time but introduces a registry-state coupling on the hot path; option (a) defers detection but matches the spec's literal worker-pool-panics contract.

**Patch (apply now):**

- [x] [Review][Patch] **`IronDefer::enqueue/enqueue_at/find/list` missing `#[instrument]` decoration** [`crates/api/src/lib.rs:184-245`] — The Critical Conventions section in Dev Notes explicitly names `IronDefer::enqueue` as one of the methods that must `skip(self, payload)` for FR38 privacy, which presupposes a `#[instrument]` attribute. Architecture lines 692–702 mandate `#[instrument(skip(self), fields(...), err)]` on every public async method in the application/infrastructure layers; the api crate's library facade should follow the same convention so spans propagate correctly into the binary's tracing pipeline. None of the public async methods on `IronDefer` carry `#[instrument]` today. Fix: add `#[instrument(skip(self, task), fields(queue = %queue, kind = %T::KIND), err)]` to `enqueue` and `enqueue_at` (skip the task itself for privacy), `#[instrument(skip(self), fields(task_id = %id), err)]` to `find`, `#[instrument(skip(self), fields(queue = %queue), err)]` to `list`. Source: `auditor`.

- [x] [Review][Patch] **`tokio` missing from `crates/api/Cargo.toml [dependencies]`** [`crates/api/Cargo.toml:17-25`] — AC 8 explicitly says "Add `tokio = { workspace = true }`" but the file only carries it under `[dev-dependencies]`. The crate currently builds because tokio is pulled in transitively via `iron-defer-infrastructure`/`tracing-subscriber`, but the spec requires the dependency to be **explicit** so the api crate's runtime requirements are visible without spelunking into transitive graphs. Source: `auditor`.

- [x] [Review][Patch] **`enqueue_persists_task_with_kind_and_default_scheduled_at` does not assert `saved.status == Pending`** [`crates/api/tests/integration_test.rs:118-149`] — AC 9 explicitly enumerates `status == Pending` as one of the assertions for this test. The test currently checks `kind`, `queue`, `attempts`, `scheduled_at`, and the payload round-trip, but never inspects `saved.status`. A regression that flipped the default status to `Running` would not be caught. Source: `edge`.

- [x] [Review][Patch] **`enqueue_rejects_invalid_queue_name` does not verify the underlying `ValidationError` is visible** [`crates/api/tests/integration_test.rs:303-307`] — AC 9 wording: "expect `Err(TaskError::InvalidPayload)` with the underlying `ValidationError` message visible." The test currently asserts only `msg.contains("invalid queue name")`, which is the static prefix added by `IronDefer::enqueue_inner` itself — the underlying `ValidationError::EmptyQueueName` text never has to appear for the assertion to pass. Fix: also assert `msg.contains("must not be empty")` and pattern-match on `matches!(err, TaskError::InvalidPayload { .. })` to verify the variant. Source: `auditor`.

- [x] [Review][Patch] **`builder_requires_pool` does not `matches!` on the `TaskError::Storage` variant** [`crates/api/tests/integration_test.rs:32-42`] — AC 9 says "expect `Err(TaskError::Storage)` with message containing 'PgPool not provided'." The test only asserts the Display string contains the substring; a refactor that switched the variant while keeping the same message would silently pass. Fix: add `assert!(matches!(err, TaskError::Storage { .. }))`. Source: `auditor`.

- [x] [Review][Patch] **`TaskHandlerAdapter::execute` has zero test coverage — story's central new code path** [`crates/api/src/lib.rs:96-115`] — The deserialize-then-execute path (architecture lines 1162–1182) is the heart of Story 1A.3's type-erased dispatch contract, but no test in the workspace exercises it. The mockall `SchedulerService` tests don't touch the registry at all; the integration tests register handlers but never call them (Epic 1B owns dispatch). Fix: add an `#[cfg(test)]` unit test in `crates/api/src/lib.rs` that constructs a `TaskHandlerAdapter::<EchoTask>(PhantomData)`, calls `.execute(&payload, &ctx)` directly, and asserts (a) the happy path returns `Ok(())` for a valid payload, (b) a malformed payload (missing field, wrong type) maps to `TaskError::InvalidPayload`. Source: `edge`.

- [x] [Review][Patch] **Dead `const _: fn() = ...` block + unused `Task` import in `scheduler.rs`** [`crates/application/src/services/scheduler.rs:18, 124-129`] — The block declares a nested `_assert_task_in_scope<T: Task>` function inside a closure body but never instantiates or calls it, so the generic parameter is never monomorphized and the assertion is dead. The stated purpose ("keep `Task` in scope for rustdoc cross-references") doesn't work as written — if `Task` were renamed/removed the block would still compile. Fix: remove the dead block AND remove the `Task` import on line 18 (it's unused outside this dead block). Source: `edge`.

- [x] [Review][Patch] **`TaskRegistry::register` silently overwrites duplicate kinds with no warning** [`crates/application/src/registry.rs:88-91`] — Architecture allows the overwrite (HashMap insert semantics, AC 2), but a `tracing::warn!(kind = handler.kind(), "overwriting previously-registered task handler")` before the insert is essentially free and turns an invisible duplicate-KIND bug into a visible warning at registration time. Without it, two unrelated `Task` types accidentally sharing a `KIND` literal silently lose the first registration. Fix: add the warn-on-overwrite. Source: `edge`.

- [x] [Review][Patch] **`builder_runs_migrations_by_default` cannot distinguish "migrated" from "skipped"** [`crates/api/tests/integration_test.rs:44-68`] — The test reuses the shared `TEST_DB` pool which `boot_test_db` already migrated via `IronDefer::migrator().run(&pool)` in `common/mod.rs:82`. The `tasks` table is therefore guaranteed to exist before the test starts, regardless of whether `IronDeferBuilder::build()` actually re-runs migrations. The `assert!(count.0 >= 0)` is also tautological (`count(*)` is always non-negative). Fix: switch this test to use `fresh_unmigrated_pool()` (like the skip-migrations test does) so the assertion "tasks table now exists" actually proves `build()` ran the migrator. Drop the tautological `assert!(count.0 >= 0)`. Source: `blind`.

- [x] [Review][Patch] **`builder_skip_migrations_does_not_run_migrator` error matcher is too broad** [`crates/api/tests/integration_test.rs:91-94`] — The matcher accepts an error containing "tasks" OR "does not exist" OR "relation". The substring "tasks" appears in the SQL string itself, so almost any sqlx error against this query (connection refused, auth failure, future syntax bug) would satisfy the assertion. Fix: match on SQLSTATE `42P01` (`undefined_table`) explicitly via `err.as_database_error().and_then(|e| e.code()).as_deref() == Some("42P01")`. Source: `blind`.

- [x] [Review][Patch] **`payload.clone()` in `TaskHandlerAdapter::execute` allocates per-dispatch on the hot path** [`crates/api/src/lib.rs:107-114`] — `serde_json::from_value::<T>(payload.clone())` deep-clones the entire JSON tree even though `serde_json::Value` implements `serde::Deserializer` *by reference*. The escape: call `T::deserialize(payload)` directly (where `payload: &serde_json::Value`), which avoids the clone entirely. The architectural rationale for the manual `Pin<Box<...>>` shape (per architecture line 1149) is "explicit boxing control on the per-task hot path" — undermining that with an unconditional payload clone is contradictory. Fix: replace `serde_json::from_value(payload.clone())` with `T::deserialize(payload)` and import `serde::Deserialize`. Sources: `blind`+`edge`.

**Deferred (real but out of 1A.3 scope):**

- [x] [Review][Defer] **Untested `serde::Error → TaskError::InvalidPayload` mapping in adapter** [`crates/api/src/lib.rs:108-112`] — The error path is partially exercised by the new `TaskHandlerAdapter` unit test (Patch #6). The integration test does not separately verify a malformed payload's error variant. Defer remaining coverage to whenever the registry is integrated end-to-end (Epic 1B worker pool tests).

- [x] [Review][Defer] **Out-of-range `scheduled_at` produces opaque `TaskError::Storage`** [`crates/api/src/lib.rs:194`] — A caller passing `DateTime::<Utc>::MAX_UTC` or similar would fail inside sqlx encoding and surface as Storage instead of `InvalidPayload`. Speculative; no realistic caller does arithmetic that overflows `chrono::DateTime`. Defer to a follow-up validation pass alongside Story 4.1's caller-input validation.

- [x] [Review][Defer] **Partial migration recovery undefined** [`crates/api/src/lib.rs:347-353`] — If `MIGRATOR.run` fails partway through a multi-statement migration, sqlx leaves the schema half-applied and records the failed migration. The library offers no `repair()` accessor and no warning about the failure mode. Defer to Epic 5 (production readiness) — production migration safety is its remit.

- [x] [Review][Defer] **`fresh_unmigrated_pool` silent skip with no log** [`crates/api/tests/common/mod.rs:59-72`] — When `get_host_port_ipv4` or `PgPool::connect` fails after the container has already started, the `?` operator propagates `None` without an `eprintln!`, so the test mysteriously skips with no diagnostic. `boot_test_db` does print on failure. Defer: matters only when Docker is flaky locally; current dev/CI setup has Docker reliably available. Quick follow-up to add the `eprintln!` lines.

- [x] [Review][Defer] **`task_trait_compiles_with_native_async_fn` is a no-op test** [`crates/domain/src/model/task.rs:268-270`] — The body is `let _ = DummyTask::KIND;`. The compile-time validation it claims to perform happens at the surrounding `impl Task for DummyTask` block, regardless of this `#[test]`. Pre-existing from Story 1A.1; not a 1A.3 regression. Defer: cleanup ticket.

- [x] [Review][Defer] **`MIGRATOR.run` error opaquely boxed into `TaskError::Storage`, losing `MigrateError` variant info** [`crates/api/src/lib.rs:347-353`] — Callers cannot programmatically react to migration failures (e.g. `VersionMismatch`, `Dirty`, `Execute`); they can only string-match. Adding a typed `TaskError::Migration { source: MigrateError }` variant would solve this but requires a domain-crate change. Defer to Epic 5 when the broader error model gets reviewed.



### Architecture Source of Truth

- **Public Library API surface (D7.1):** `docs/artifacts/planning/architecture.md` lines 466–491. The `IronDefer::builder()` chain is normative — `pool() → concurrency() → poll_interval() → sweeper_interval() → register::<T>() → build().await`. This story implements `pool()`, `register::<T>()`, `skip_migrations()`, and `build()`. The runtime tuning methods (`concurrency`, `poll_interval`, `sweeper_interval`) and `start()` are Epic 1B.
- **`Task` trait shape (D7.2):** lines 492–510. Note: this story adds the `Serialize + DeserializeOwned` bounds that 1A.1 deferred. The `TaskContext` shape in D7.2 differs from the 1A.1 implementation (see Task 6 for reconciliation).
- **`TaskHandler` trait (C4):** lines 1143–1182. The `Pin<Box<dyn Future + Send>>` return type is normative — do NOT use `#[async_trait]` on `TaskHandler`. The `TaskHandlerAdapter<T>` bridge code lines 1162–1182 is the literal implementation pattern; copy it.
- **TaskRegistry ownership rule:** lines 665–667 + 778. Construction is forbidden outside the api crate. The application crate exposes the type so it can be injected as a dependency.
- **Public API boundary:** lines 680–683 + 936–939. `crates/api/src/lib.rs` may NOT re-export `axum`, `reqwest`, or any `sqlx` type **except** `sqlx::PgPool` as a builder INPUT (and `&'static sqlx::migrate::Migrator` as the `migrator()` accessor return — architecture line 1140 explicitly authorizes this).
- **Embedded migrations:** lines 1131–1141. `MIGRATOR.run(&pool).await` runs in `IronDeferBuilder::build()` unless `.skip_migrations(true)`. The `IronDefer::migrator() -> &'static sqlx::migrate::Migrator` accessor surfaces the migration set so callers can inspect or run it inside their own transaction.
- **No logic in `lib.rs` rule scope:** lines 562–565. `crates/api/src/lib.rs` is the SOLE exception. Domain, application, and infrastructure all keep `lib.rs` to re-exports only.
- **`#[instrument]` rule:** lines 692–702. Every public async method in application + infrastructure layers gets `#[instrument(skip(self), fields(...), err)]`. `payload` is NEVER in `fields(...)` (FR38, line 701).
- **`mockall` for application port traits (C6):** lines 1194–1199. Add `mockall` as an application dev-dep, annotate port traits with `#[automock]`.

### Critical Conventions (do NOT deviate)

- **`TaskRegistry::new()` is constructed in `crates/api/src/lib.rs` ONLY.** If a future story needs a registry in application or infrastructure for testing, use a mock — never construct a real one.
- **`crates/api/src/lib.rs` is the SOLE crate where logic in `lib.rs` is permitted.** This is enforced by convention only (no compiler check). Keep `domain/src/lib.rs`, `application/src/lib.rs`, `infrastructure/src/lib.rs` to re-exports only. The "logic" in `api/src/lib.rs` is strictly the `IronDefer` builder + facade — anything more complex moves to a sibling module (`config.rs`, `shutdown.rs`, etc., per architecture line 891+).
- **Public API boundary discipline:** the only `sqlx` type the public API may mention is `PgPool` (as a builder input) and `&'static sqlx::migrate::Migrator` (as the `migrator()` accessor return). No `sqlx::Error`, no `sqlx::Pool<Postgres>` (use the `PgPool` alias). No `axum::*`, no `reqwest::*`, no `iron_defer_infrastructure::PostgresTaskRepository`.
- **Task trait serde bounds enable type-erased dispatch.** With `T: Serialize + DeserializeOwned`, the `TaskHandlerAdapter<T>` can roundtrip `T → JSON → T` for the registry's hot-path execution. Without these bounds, the registry would have to store the user's concrete type which defeats the type-erasure pattern.
- **Payload privacy in `#[instrument]`:** `payload` is never in `fields(...)`. The `SchedulerService::enqueue` and `IronDefer::enqueue` methods both `skip(self, payload)` (or equivalent) so the `serde_json::Value` does not appear in trace events. FR38 default. The `WorkerConfig::log_payload = true` opt-in is wired up in Epic 3, not here.
- **Builder consumes self:** all `IronDeferBuilder` chain methods take `mut self` and return `Self`, not `&mut self`. This matches the canonical Rust builder pattern and the architecture example at line 471.
- **Builder NEVER spawns a Tokio runtime.** Only `await` on the caller's runtime. Anything that smells like `tokio::runtime::Runtime::new()` is a story-killing bug — architecture line 776 forbids it explicitly.
- **No `unwrap()` / `expect()` / `panic!()` in `src/`** except inside `#[cfg(test)]` blocks. The `register::<T>()` method does NOT panic on duplicate kinds (HashMap insert overwrites silently, which is acceptable per AC 2 — overwrite is the convention). Panic-on-missing-kind is Epic 1B's worker pool's responsibility.

### Out of Scope for This Story

- **`IronDefer::start()`** — Epic 1B (worker pool + sweeper + claim loop).
- **`engine.cancel(id)`** — Story 4.1.
- **REST API / `axum` handlers** — Story 1B.3 / Epic 4.
- **CLI / `clap` commands** — Story 4.3.
- **`figment` config loading** — later story; `crates/api/src/config.rs` does NOT need to exist yet.
- **Sweeper / zombie recovery** — Epic 2.
- **OTel metrics emission** — Epic 3.
- **Caller-supplied idempotency keys** — explicitly deferred (see Task 9).
- **`engine.list(queue, filters)`** — the AC says `list(queue, filters)` but this story implements only `list(queue)` (no filter object). Filters (status, time range, pagination) are deferred to Story 4.2 (`GET /tasks` listing). Document this in the AC text and the public API doc.
- **Adding `concurrency`, `poll_interval`, `sweeper_interval` builder methods** — Epic 1B.
- **Worker pool, JoinSet, Semaphore, CancellationToken** — Epic 1B.

### Tooling Notes — `mockall::automock` on async traits

- `mockall` works with `#[async_trait]` traits since v0.11. The annotation order matters: `#[cfg_attr(test, mockall::automock)]` goes ABOVE `#[async_trait]`:
  ```rust
  #[cfg_attr(test, mockall::automock)]
  #[async_trait]
  pub trait TaskRepository: Send + Sync + 'static {
      async fn save(&self, task: &TaskRecord) -> Result<TaskRecord, TaskError>;
      // ...
  }
  ```
- The generated `MockTaskRepository` lives in scope as `crate::ports::task_repository::MockTaskRepository` (or wherever the trait is defined). Import it in tests with `use crate::ports::task_repository::MockTaskRepository;`.
- **Setting expectations on async methods:** `mock.expect_save().returning(|_| Ok(synthetic_record()));` works directly. To capture arguments, use `.withf(|task| task.kind == "echo")` or `.with(eq(specific_record))`.
- **Returning `Result<Option<T>, E>`:** `mock.expect_find_by_id().returning(|_| Ok(None));` and `Ok(Some(record))` both work fine.

### Tooling Notes — `serde_json::to_value` failure modes

- `serde_json::to_value(task)` can fail if `T`'s `Serialize` impl returns an error. For derived `Serialize` on a struct with serde-supported field types, this is effectively impossible — but the API must still return a `Result`.
- **Map serde errors to `TaskError::InvalidPayload { reason }`** — same as `from_value` failures in `TaskHandlerAdapter`. Both directions of the JSON conversion use the same error variant.
- Do NOT `.unwrap()` the conversion. The `enqueue` method must return `Result<TaskRecord, TaskError>` cleanly.

### Tooling Notes — `IronDeferBuilder::build` migration translation

- `MIGRATOR.run(&pool).await` returns `Result<(), sqlx::migrate::MigrateError>`. This error type is NOT `sqlx::Error` and does NOT have a free `From` impl into `PostgresAdapterError::Query`.
- **Translation pattern:** wrap the migrate error in a `Box<dyn std::error::Error + Send + Sync>` and feed it directly into `TaskError::Storage`:
  ```rust
  if !self.skip_migrations {
      iron_defer_infrastructure::MIGRATOR
          .run(&pool)
          .await
          .map_err(|e| TaskError::Storage { source: Box::new(e) })?;
  }
  ```
- This bypasses `PostgresAdapterError` entirely for this one call site, which is acceptable: `MigrateError` is a separate sqlx error family from `sqlx::Error` and the translation through `PostgresAdapterError::Query { #[from] sqlx::Error }` would not type-check anyway.

### Previous Story Intelligence (from Story 1A.2)

- **`TaskRecord` is `#[non_exhaustive]`** — constructed via `pub const fn TaskRecord::new(14 args)`. The `SchedulerService::enqueue` and `IronDefer::enqueue` paths both go through this constructor.
- **`PostgresTaskRepository::new(pool: PgPool) -> Self`** is the adapter constructor — already implemented and tested by 1A.2. `IronDeferBuilder::build` wires it up: `Arc::new(PostgresTaskRepository::new(pool.clone())) as Arc<dyn TaskRepository>`.
- **`iron_defer_infrastructure::MIGRATOR`** is a `pub static sqlx::migrate::Migrator` that 1A.3's builder runs unless `skip_migrations(true)`.
- **`iron_defer_infrastructure::create_pool(&DatabaseConfig)`** exists but is NOT used by `IronDeferBuilder` — the builder receives a caller-provided `PgPool` directly. `create_pool` is for the standalone-binary path (Epic 4 / Story 4.3).
- **`TaskError::Storage` carries `#[source] source: Box<dyn std::error::Error + Send + Sync>`** — preserves the source chain. The 1A.3 builder's migration-error translation uses this exact pattern.
- **`TaskError::NotFound` does NOT exist** (removed in 1A.2). Absence is `Option::None`. `find()` returns `Result<Option<TaskRecord>, TaskError>`.
- **`QueueName::try_from(&str)` returns `Result<QueueName, ValidationError>`** with error variants: `EmptyQueueName`, `QueueNameWhitespace`, `QueueNameForbiddenChar`, `QueueNameTooLong { length, max }`. `ValidationError` does NOT implement `Into<TaskError>` automatically — `IronDefer::enqueue` must do the translation explicitly: `.map_err(|e| TaskError::InvalidPayload { reason: e.to_string() })`.
- **Migration uses CHECK constraints** for `status IN (...)` and `length(kind) > 0` (added by 1A.2 code review). The `SchedulerService::enqueue` path produces tasks with `status = Pending` and `kind = T::KIND` (a `&'static str`, never empty for valid task types) so the constraints are non-issues for normal flow but still backstop direct DB access.
- **Test count baseline:** 44 tests passing (18 domain + 20 infra unit + 6 infra integration). Story 1A.3 adds: ~5 SchedulerService mock tests + ~3 TaskRegistry tests + 1 DummyTask serde smoke test + ~9 api integration tests = ~62 total target.
- **Existing `crates/api/src/lib.rs` placeholder** — `pub fn run_placeholder()` is currently called by `main.rs`. Story 1A.3 leaves it in place (with `#[allow(dead_code)]` if needed) — the binary's full wiring is Epic 4. Real `IronDefer` builder is added alongside the placeholder, not replacing it.
- **`crates/api/src/main.rs` already initializes `tracing-subscriber`** (1A.1 patch). The new `IronDefer` engine's `#[instrument]` spans will automatically appear in the binary's log output once the binary is wired up to use it (Epic 4).

### Project Structure Notes

- New files:
  - `crates/application/src/registry.rs` (TaskHandler trait + TaskRegistry struct)
  - `crates/application/src/services/mod.rs` (declares `scheduler` submodule)
  - `crates/application/src/services/scheduler.rs` (SchedulerService impl + mockall unit tests)
  - `crates/api/tests/common/mod.rs` (testcontainers helper)
  - `crates/api/tests/integration_test.rs` (9 integration tests)
- Modified files:
  - `Cargo.toml` (workspace root) — add `mockall = "0.13"` to `[workspace.dependencies]`
  - `crates/domain/src/model/task.rs` — add `Serialize + DeserializeOwned` bounds to `Task`; update `DummyTask` test fixture; add doc comment to `TaskContext` explaining D7.2 deviation
  - `crates/application/Cargo.toml` — add `chrono`, `serde_json` deps; add `[dev-dependencies]` with `mockall`
  - `crates/application/src/lib.rs` — declare `services` and `registry` modules; re-export `SchedulerService`, `TaskHandler`, `TaskRegistry`
  - `crates/application/src/ports/task_repository.rs` — add `#[cfg_attr(test, mockall::automock)]`
  - `crates/application/src/ports/task_executor.rs` — add `#[cfg_attr(test, mockall::automock)]`
  - `crates/api/Cargo.toml` — add `sqlx`, `chrono`, `serde_json` deps; add `[dev-dependencies]` for tests
  - `crates/api/src/lib.rs` — replace placeholder content with the real `IronDefer` builder, `IronDeferBuilder`, `TaskHandlerAdapter`, public re-exports. Keep `run_placeholder()` (with `#[allow(dead_code)]`) until Epic 4 rewires the binary.
  - `docs/artifacts/implementation/deferred-work.md` — mark Task-trait-serde-bounds RESOLVED; mark duplicate-id PARTIALLY RESOLVED (Task 9)
  - `docs/artifacts/implementation/sprint-status.yaml` — `1a-3-...: backlog → ready-for-dev → in-progress → review → done`

### References

- [Source: docs/artifacts/planning/architecture.md#D7.1-Embedded-Library-Entry-Point (lines 466–491)]
- [Source: docs/artifacts/planning/architecture.md#D7.2-Task-Trait (lines 492–510)]
- [Source: docs/artifacts/planning/architecture.md#C4-TaskHandler-pattern (lines 1143–1182)]
- [Source: docs/artifacts/planning/architecture.md#TaskRegistry-ownership (lines 665–667, 778)]
- [Source: docs/artifacts/planning/architecture.md#Public-API-Boundary (lines 680–683, 936–939)]
- [Source: docs/artifacts/planning/architecture.md#Embedded-migrations (lines 1131–1141)]
- [Source: docs/artifacts/planning/architecture.md#mockall-rule (lines 1194–1199)]
- [Source: docs/artifacts/planning/epics.md#Story-1A.3 (lines 324–364)]
- [Source: docs/adr/0001-hexagonal-architecture.md]
- [Source: docs/adr/0002-error-handling.md]
- [Source: docs/artifacts/implementation/1a-2-postgres-schema-and-task-repository.md (full file — provides repository constructor signature, MIGRATOR, error handling patterns, testcontainers setup template)]
- [Source: docs/artifacts/implementation/deferred-work.md (Task-trait-serde-bounds + duplicate-id items resolved here)]

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context)

### Debug Log References

- `cargo check --workspace` — clean (4 crates)
- `SQLX_OFFLINE=true cargo check --workspace` — clean (no new sqlx queries; existing offline cache covers everything)
- `cargo fmt --all -- --check` — clean (after one auto-format pass during dev)
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean (3 doc-markdown fixes during dev: backticks around `HashMap`, `register::<T>()`, and `PostgreSQL`)
- `SQLX_OFFLINE=true cargo test --workspace` — **67 passed, 0 failed**, 2 doctests ignored:
  - 19 domain unit (was 18, +1 for `task_trait_serde_round_trips`)
  - 11 application unit (was 1, +5 registry tests + 5 SchedulerService mockall tests)
  - 20 infrastructure unit (unchanged)
  - 6 infrastructure integration (unchanged)
  - 2 api unit (was 0, +2 builder tests)
  - 9 api integration (NEW — testcontainers via the public `IronDefer::migrator()` path)
- `cargo deny check bans` — `bans ok`
- `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (production graph clean across the api crate too)

### Completion Notes List

- All 11 ACs satisfied. Builder → enqueue → find → list → migrator end-to-end loop is closed and exercised by 9 testcontainers-backed integration tests.
- **`Task` trait serde supertraits added (AC 1).** `pub trait Task: Send + Sync + Serialize + DeserializeOwned + 'static` per architecture §D7.2. Inline `DummyTask` test fixture updated to `#[derive(Serialize, Deserialize)]`. New `task_trait_serde_round_trips` smoke test confirms the bounds permit `serde_json::to_value`/`from_value` round-trip. **Resolves a deferred-work item from Story 1A.1.**
- **`TaskHandler` + `TaskRegistry` (AC 2)** in `crates/application/src/registry.rs`. Manual `Pin<Box<dyn Future + Send>>` return type per architecture C4 — explicitly NOT `#[async_trait]`, because the per-task hot path runs once per claimed task and we want explicit control over the boxing. `TaskRegistry` exposes `new()`, `register()`, `get()`, `len()`, `is_empty()`. Lookups return `None` for unregistered kinds — Epic 1B's worker pool is responsible for turning `None` into a panic at the dispatch site, the registry itself never panics. 5 unit tests cover empty registry, registration, overwrite-on-duplicate-kind, and the `Debug` impl.
- **`SchedulerService` (AC 3)** in `crates/application/src/services/scheduler.rs`. Wraps `Arc<dyn TaskRepository>`. `enqueue` constructs a `TaskRecord` with `TaskId::new()`, `status = Pending`, `attempts = 0`, `priority = 0`, `max_attempts = 3`, `scheduled_at = scheduled_at.unwrap_or_else(Utc::now)`. The `created_at`/`updated_at` placeholders are overwritten by Postgres `DEFAULT now()` per Story 1A.2's `save()` contract. All public async methods carry `#[instrument(skip(self), fields(queue, kind / task_id), err)]` with `payload` skipped for FR38 privacy.
- **mockall integration (AC 4)** — `mockall = "0.13"` added to `[workspace.dependencies]` and `crates/application/Cargo.toml` `[dev-dependencies]`. `TaskRepository` and `TaskExecutor` decorated with `#[cfg_attr(test, mockall::automock)]`. The 5 SchedulerService unit tests use `MockTaskRepository::new()` with `expect_save().withf(...)`/`expect_find_by_id().with(eq(...))` predicates to assert call arguments. No real database needed.
- **`TaskHandlerAdapter<T>` (AC 5)** in `crates/api/src/lib.rs` — `pub(crate)` (never crosses the public API boundary). Implements `TaskHandler` for any `T: Task` by deserializing payload via `serde_json::from_value(payload.clone())` and forwarding to `T::execute(ctx)`. serde errors map to `TaskError::InvalidPayload { reason }`. Architecture lines 1162–1182 implementation pattern followed verbatim.
- **`IronDefer` + `IronDeferBuilder` (AC 6)** in `crates/api/src/lib.rs`. Builder methods: `pool()`, `register::<T>()`, `skip_migrations()`, `build()`. Engine methods: `enqueue<T>()`, `enqueue_at<T>()`, `find()`, `list()`, `pool()`, `registry()`, `migrator()`. The `enqueue_inner` private helper consolidates the queue-name validation + JSON serialization + scheduler delegation logic so `enqueue` and `enqueue_at` differ only in their `Option<DateTime<Utc>>` argument. The builder consumes `mut self` per the canonical Rust pattern.
- **Migration error translation (AC 6 / Tooling Notes)** — `MIGRATOR.run(&pool).await` returns `Result<(), sqlx::migrate::MigrateError>` which is NOT `sqlx::Error` and has no free `From` impl through `PostgresAdapterError::Query`. The builder wraps `MigrateError` directly via `Box::new(e)` into `TaskError::Storage { source: ... }`, preserving the source chain. Confirmed by inspection of the source-chain walk in test asserts.
- **Public API re-exports (AC 7).** Re-exported: `iron_defer_application::{TaskHandler, TaskRegistry}`, `iron_defer_domain::{QueueName, Task, TaskContext, TaskError, TaskId, TaskRecord, TaskStatus}`. **Forbidden re-exports verified absent:** no `sqlx::Error`, no `axum::*`, no `reqwest::*`, no `iron_defer_infrastructure::PostgresTaskRepository`, no `PostgresAdapterError`. The only `sqlx` types crossing the boundary are `PgPool` (builder input) and `&'static sqlx::migrate::Migrator` (returned by `IronDefer::migrator()` per architecture line 1140).
- **`TaskContext` reconciliation (Task 6).** Added a doc comment explaining the deviation from architecture §D7.2 — Story 1A.1 chose `worker_id` + `attempt: i32` because Epic 1B's executor needs the worker identity threaded through and `i32` matches `TaskRecord::attempts`. Story 1A.3 leaves the shape unchanged and documents the decision so future story authors don't re-litigate it.
- **9 api integration tests (AC 9)** in `crates/api/tests/integration_test.rs` exercise the full builder + engine surface against a fresh testcontainers Postgres. The shared `OnceCell<Option<TestDb>>` pattern from 1A.2 was copied into `crates/api/tests/common/mod.rs`, plus a new `fresh_unmigrated_pool()` helper for the `builder_skip_migrations_does_not_run_migrator` test (which can't share the pre-migrated container). Migrations in the shared pool helper run via `IronDefer::migrator().run(&pool)` — the public API path — so the migrator accessor is exercised as part of the suite.
- **Idempotency / duplicate-id resolution (Task 9).** Decision recorded: `IronDefer::enqueue<T>` always generates a fresh `TaskId::new()` internally; the public library API has no surface for caller-supplied ids. The deferred-work item is therefore unreachable from supported callers. Caller-supplied idempotency keys are deferred to the REST API / CLI stories where HTTP request deduplication becomes a real concern. `deferred-work.md` updated to PARTIALLY RESOLVED.
- **Notable architecture invariant enforced:** the workspace-wide `TaskRegistry::new()` constructor is called in EXACTLY ONE place: `IronDeferBuilder::default()` in `crates/api/src/lib.rs`. Verified by grep across `crates/`. The application crate exposes the type via `pub use crate::registry::{TaskHandler, TaskRegistry}` so it can be injected as `Arc<TaskRegistry>` into worker pools and sweepers in Epic 1B/2.1, but it never instantiates one itself. Test code in `crates/application/src/registry.rs::tests` does call `TaskRegistry::new()`, which is acceptable per architecture lines 665–667 ("constructed in the api crate" applies to production code paths).
- **Test count climb:** Story 1A.2 baseline was 44 tests passing. Story 1A.3 adds: +1 domain (Task serde smoke test), +10 application (5 registry + 5 mockall scheduler), +2 api unit (builder default + skip_migrations setter), +9 api integration → **67 tests total**, exceeding the AC 11 target of ~62.
- **Note for the next story:** `IronDefer::start()` (worker pool entry point), `register_with_arc::<T>()` (alternate registration), task cancellation via `engine.cancel(id)`, and `engine.list(queue, filters)` filter object — all Epic 1B / Epic 4 territory. Story 1A.3 left clean seams: `engine.registry()` returns `&Arc<TaskRegistry>` so a worker pool can clone it without re-walking the builder.

### File List

**New files:**

- `crates/application/src/registry.rs`
- `crates/application/src/services/mod.rs`
- `crates/application/src/services/scheduler.rs`
- `crates/api/tests/common/mod.rs`
- `crates/api/tests/integration_test.rs`

**Modified files:**

- `Cargo.toml` (workspace root) — added `mockall = "0.13"` to `[workspace.dependencies]` (Dev section)
- `crates/domain/src/model/task.rs` — added `Serialize + DeserializeOwned` supertrait bounds to `Task`; updated `DummyTask` test fixture with `#[derive(Serialize, Deserialize)]`; added `task_trait_serde_round_trips` smoke test; added `TaskContext` deviation doc comment explaining the 1A.1 choice vs architecture §D7.2
- `crates/application/Cargo.toml` — added `chrono`, `serde_json`, `tracing` workspace deps; added `[dev-dependencies]` with `mockall` and `tokio` (macros + rt-multi-thread)
- `crates/application/src/lib.rs` — declared `registry` and `services` modules; re-exported `TaskHandler`, `TaskRegistry`, `SchedulerService`
- `crates/application/src/ports/task_repository.rs` — added `#[cfg_attr(test, mockall::automock)]` annotation
- `crates/application/src/ports/task_executor.rs` — added `#[cfg_attr(test, mockall::automock)]` annotation
- `crates/api/Cargo.toml` — added `chrono`, `serde_json`, `sqlx` workspace deps; added `[dev-dependencies]` with `serde`, `testcontainers`, `testcontainers-modules`, `tokio`, `uuid`
- `crates/api/src/lib.rs` — replaced placeholder content with the real `IronDefer` engine, `IronDeferBuilder`, `TaskHandlerAdapter`, public re-exports. `run_placeholder()` retained with `#[allow(dead_code)]` so the binary entry point keeps compiling until Epic 4.
- `docs/artifacts/implementation/deferred-work.md` — marked `Task` trait serde supertraits item RESOLVED in 1A.3; marked duplicate-id item PARTIALLY RESOLVED with the "enqueue assigns fresh ids" reasoning
- `docs/artifacts/implementation/sprint-status.yaml` — `1a-3-...: backlog → ready-for-dev → in-progress → review`

### Change Log

- 2026-04-06 — Story 1A.3 implemented. `Task` trait gained `Serialize + DeserializeOwned` supertraits (resolving 1A.1 deferred item). New `crates/application/src/registry.rs` (`TaskHandler` trait + `TaskRegistry` struct, manual `Pin<Box<Future + Send>>` per architecture C4). New `crates/application/src/services/scheduler.rs` (`SchedulerService` with enqueue/find/list + 5 mockall unit tests). `TaskRepository` and `TaskExecutor` annotated with `#[cfg_attr(test, mockall::automock)]`. `crates/api/src/lib.rs` rewritten to ship the real `IronDefer` engine + `IronDeferBuilder` + `TaskHandlerAdapter<T>` per architecture §D7.1 and lines 1162–1182. New 9-test api integration suite (`crates/api/tests/integration_test.rs`) exercises builder, migrations, skip_migrations, enqueue/enqueue_at, find/find_None, list_by_queue, queue-name validation. Public API surface re-exports verified — no forbidden `sqlx`/`axum`/`reqwest`/infrastructure types leak. `TaskContext` deviation from architecture §D7.2 documented. Idempotency / duplicate-id deferred item PARTIALLY RESOLVED via "enqueue always generates fresh `TaskId`". 67 tests passing (up from 44). All quality gates green: `cargo fmt --check`, `cargo clippy -- -D clippy::pedantic`, `cargo test --workspace`, `cargo deny check bans`, production graph still rustls-only. Status: ready-for-dev → in-progress → review.
- 2026-04-06 — Code review (3-layer adversarial: Blind Hunter, Edge Case Hunter, Acceptance Auditor) → 1 decision resolved + 11 patches applied + 6 items deferred + 2 dismissed.
  - **Decision resolved (option b — tighten enqueue):** `IronDefer::enqueue_inner` now checks `self.registry.get(T::KIND).is_none()` before persistence and returns `TaskError::InvalidPayload` with the missing-kind context (including `std::any::type_name::<T>()` for ergonomic developer feedback). Typos in handler registration are now caught at the enqueue surface instead of crashing the Epic 1B worker pool at dispatch time.
  - **Patches applied:** (1) `#[instrument(skip(self, task), fields(queue, kind, ...), err)]` added to `IronDefer::enqueue/enqueue_at/find/list` per Critical Convention. (2) `tokio = { workspace = true }` added to `crates/api/Cargo.toml [dependencies]` per AC 8 literal. (3) `assert_eq!(saved.status, TaskStatus::Pending)` added to `enqueue_persists_task...` test per AC 9. (4) `enqueue_rejects_invalid_queue_name` now `matches!(err, TaskError::InvalidPayload { .. })` AND asserts the underlying `ValidationError` text "must not be empty" is visible. (5) `builder_requires_pool` now `matches!(err, TaskError::Storage { .. })`. (6) **3 new `TaskHandlerAdapter` unit tests** in `crates/api/src/lib.rs#[cfg(test)]`: `kind` round-trip, happy-path execute, malformed-payload `serde::Error → TaskError::InvalidPayload` mapping. (7) Removed dead `const _: fn() = ...` block + unused `Task` import in `scheduler.rs`. (8) `TaskRegistry::register` now emits `tracing::warn!` on duplicate-kind overwrite (keeping the HashMap-insert semantics but making the surprise visible). (9) `builder_runs_migrations_by_default` rewritten to use `fresh_unmigrated_pool()` — pre-condition asserts the table does NOT exist, post-condition asserts it does, actually proving the migrator ran. (10) `builder_skip_migrations_does_not_run_migrator` error matcher tightened to `SQLSTATE 42P01 (undefined_table)` instead of substring matching. (11) `TaskHandlerAdapter::execute` now uses `T::deserialize(payload)` (by-reference deserialization via `&serde_json::Value`'s `Deserializer` impl) instead of `serde_json::from_value(payload.clone())` — eliminates per-task hot-path payload clone, honoring the architectural rationale for the manual `Pin<Box<...>>` shape (line 1149).
  - **Test suite climb:** 67 → **70 tests passing** (3 new `TaskHandlerAdapter` unit tests). Pre-existing test counts unchanged.
  - **6 deferred items** added to `deferred-work.md` under "code review of 1a-3" heading: untested serde-error integration coverage (Epic 1B), out-of-range `scheduled_at` (Story 4.1), partial migration recovery (Epic 5), `fresh_unmigrated_pool` silent skip (diagnostic follow-up), `task_trait_compiles_with_native_async_fn` no-op (1A.1 cleanup ticket), `MIGRATOR.run` opaque error wrapping (Epic 5).
  - Status: review → done.
