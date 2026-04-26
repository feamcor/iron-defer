# Story 6.9: Builder Pattern & Dependency Setup

Status: done

## Story

As a developer,
I want large constructors replaced with builder patterns,
so that constructing domain objects and services is ergonomic and resistant to argument-order bugs.

## Acceptance Criteria

1. **`bon` crate dependency (CR46)**

   **Given** the `bon` crate added to `[workspace.dependencies]`
   **When** I verify the dependency
   **Then** `bon`'s license is added to the `deny.toml` allow-list (if not already covered)
   **And** `cargo deny check` passes with the new dependency

2. **`TaskRecord` builder (CR46)**

   **Given** `TaskRecord` in `crates/domain/src/model/task.rs`
   **When** I inspect its constructor
   **Then** the 14-argument constructor is replaced with a `#[derive(bon::Builder)]` or equivalent builder pattern
   **And** the `#[allow(clippy::too_many_arguments)]` annotation is removed
   **And** all test helpers (e.g., `synthetic_record()`) are updated to use the builder

3. **`WorkerService::new()` builder (CR46)**

   **Given** `WorkerService::new()` in `crates/application/src/services/worker.rs`
   **When** I inspect its signature
   **Then** the 6+ argument constructor uses a builder pattern
   **And** required vs optional parameters are clearly distinguished

4. **`dispatch_task` context struct (CR46)**

   **Given** the `dispatch_task` function signature
   **When** I inspect its parameters
   **Then** related parameters are grouped into a context struct (e.g., `DispatchContext`) to reduce argument count
   **And** the function takes 4 or fewer direct parameters

5. **Combined verification**

   **Given** all builder changes
   **When** `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` run
   **Then** all tests pass and no clippy warnings are introduced

## Tasks / Subtasks

- [x] **Task 1: Add `bon` to workspace dependencies** (AC: 1)
  - [x] 1.1: Add `bon = "3"` (or latest stable) to `[workspace.dependencies]` in root `Cargo.toml`
  - [x] 1.2: Add `bon = { workspace = true }` to `[dependencies]` in `crates/domain/Cargo.toml`
  - [x] 1.3: Add `bon = { workspace = true }` to `[dependencies]` in `crates/application/Cargo.toml`
  - [x] 1.4: Verify `bon`'s license (MIT/Apache-2.0) is already covered by `deny.toml` allow-list — both MIT and Apache-2.0 are already listed
  - [x] 1.5: Run `cargo deny check` — passes

- [x] **Task 2: Convert `TaskRecord` to builder pattern** (AC: 2)
  - [x] 2.1: Add `#[derive(bon::Builder)]` to `TaskRecord` struct at `crates/domain/src/model/task.rs:77`
  - [x] 2.2: Remove `#[allow(clippy::too_many_arguments)]` from the `new()` constructor (line 108)
  - [x] 2.3: Keep `TaskRecord::new()` as a manual constructor alongside the builder for backward compatibility (or remove if all call sites are migrated)
  - [x] 2.4: Update all 6 construction sites to use the builder (see Dev Notes for full list)
  - [x] 2.5: Update `synthetic_record()` in scheduler tests (`scheduler.rs:230–248`)
  - [x] 2.6: Update `synthetic_record()` in worker tests (`worker.rs:953–971`)
  - [x] 2.7: Update `sample_task_with()` in infrastructure tests (`task_repository_test.rs:28–50`)
  - [x] 2.8: Verify all tests that construct `TaskRecord` compile and pass

- [x] **Task 3: Convert `WorkerService` to builder pattern** (AC: 3)
  - [x] 3.1: Add `#[derive(bon::Builder)]` to `WorkerService` struct at `worker.rs:47` OR create a manual builder
  - [x] 3.2: Mark required fields: `repo`, `registry`, `config`, `queue`, `token`, `worker_id`
  - [x] 3.3: Mark optional fields with defaults: `is_saturation` (default: `Arc::new(|_| false)`), `metrics` (default: `None`), `active_tasks` (default: `Arc::new(AtomicU32::new(0))`)
  - [x] 3.4: Remove `with_saturation_classifier()` and `with_metrics()` chainable methods — the builder subsumes them
  - [x] 3.5: Update the production call site in `crates/api/src/lib.rs:430–437`
  - [x] 3.6: Update all 11 test call sites in `worker.rs`

- [x] **Task 4: Create `DispatchContext` struct for `dispatch_task`** (AC: 4)
  - [x] 4.1: Define an OWNED `DispatchContext` struct (not borrowed — it must be `Clone + Send + 'static` to move into `join_set.spawn(async move { ... })`):
    ```rust
    #[derive(Clone)]
    struct DispatchContext {
        repo: Arc<dyn TaskRepository>,
        registry: Arc<TaskRegistry>,
        worker_id: WorkerId,
        base_delay_secs: f64,
        max_delay_secs: f64,
        log_payload: bool,
        metrics: Option<Metrics>,  // Metrics is Clone
        queue_str: String,
    }
    ```
    Construct once before the loop from `self` fields (cloning Arcs). Clone into each spawn closure.
  - [x] 4.2: Change `dispatch_task` signature to `async fn dispatch_task(task: &TaskRecord, ctx: &DispatchContext)`
  - [x] 4.3: Remove `#[allow(clippy::too_many_arguments)]` annotation (line 382)
  - [x] 4.4: Update the call site in `run_poll_loop` (lines 250–261) to construct `DispatchContext` before spawn
  - [x] 4.5: Update internal references from individual parameters to `ctx.repo`, `ctx.registry`, etc.

- [x] **Task 5: Verify no regressions** (AC: 5)
  - [x] 5.1: `cargo test --workspace` — all tests pass
  - [x] 5.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings (specifically no `too_many_arguments`)
  - [x] 5.3: `cargo fmt --check` — clean
  - [x] 5.4: `cargo deny check` — passes with `bon` dependency

## Dev Notes

### Architecture Compliance

- **Crate boundaries (architecture lines 924–937):** `bon` is added to `domain` and `application` crates. The `api` crate already has its own `IronDeferBuilder` — no `bon` needed there.
- **Enforcement guidelines (architecture lines 758–780):** After this story, zero `#[allow(clippy::too_many_arguments)]` annotations should remain in the codebase.

### Critical Implementation Guidance

**`bon` crate overview:**

`bon` provides `#[derive(Builder)]` that generates a type-safe builder with:
- Required fields (no default) — must be set before `.build()`
- Optional fields (`Option<T>`) — automatically optional in the builder
- Custom defaults via `#[builder(default = expr)]`
- Consuming `.build()` method returning the struct

**`TaskRecord` builder approach:**

The struct at `task.rs:77–92` has 15 fields. With `bon::Builder`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[non_exhaustive]
pub struct TaskRecord {
    pub id: TaskId,
    pub queue: QueueName,
    pub kind: TaskKind,
    pub payload: serde_json::Value,
    pub status: TaskStatus,
    pub priority: Priority,
    pub attempts: AttemptCount,
    pub max_attempts: MaxAttempts,
    #[builder(default)]
    pub last_error: Option<String>,
    pub scheduled_at: DateTime<Utc>,
    #[builder(default)]
    pub claimed_by: Option<WorkerId>,
    #[builder(default)]
    pub claimed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Optional fields (`Option<T>`) with `#[builder(default)]` don't need to be set. Required fields must all be set before `.build()`.

**Important `bon` + `#[non_exhaustive]` interaction:** `#[non_exhaustive]` prevents external construction via struct literals. The `bon` builder is generated INSIDE the defining crate, so it CAN construct the struct. External crates use the builder — which is exactly the desired behavior.

**Important `bon` + `serde` interaction:** `#[derive(bon::Builder)]` and `#[derive(Serialize, Deserialize)]` are independent derives. They should coexist without conflict. Verify during implementation.

**Migration of `TaskRecord::new()` constructor:**

After adding `#[derive(bon::Builder)]`, the manual `new()` constructor (lines 108–141) can be REMOVED — the builder replaces it. All 6 construction sites migrate:

| Site | File | Lines | Migration |
|------|------|-------|-----------|
| `SchedulerService::enqueue` | `scheduler.rs` | 84 | Use builder |
| `SchedulerService::enqueue_raw` | `scheduler.rs` | 149 | Use builder |
| `synthetic_record()` scheduler | `scheduler.rs` | 232 | Use builder |
| `synthetic_record()` worker | `worker.rs` | 955 | Use builder |
| `sample_task_with()` infra | `task_repository_test.rs` | 34 | Use builder |
| `TryFrom<TaskRow>` adapter | `postgres_task_repository.rs` | 127 | Use builder |

Example migration for `synthetic_record` (worker tests):

```rust
// OLD (14 positional args):
fn synthetic_record(kind: &str) -> TaskRecord {
    TaskRecord::new(
        TaskId::new(), queue, TaskKind::try_from(kind).unwrap(),
        json!({}), TaskStatus::Running, Priority::new(0).unwrap(),
        AttemptCount::new(1).unwrap(), MaxAttempts::new(3).unwrap(),
        None, Utc::now(), Some(WorkerId::new()), Some(Utc::now()),
        Utc::now(), Utc::now(),
    )
}

// NEW (builder):
fn synthetic_record(kind: &str) -> TaskRecord {
    let now = Utc::now();
    TaskRecord::builder()
        .id(TaskId::new())
        .queue(QueueName::try_from("test-queue").unwrap())
        .kind(TaskKind::try_from(kind).unwrap())
        .payload(json!({}))
        .status(TaskStatus::Running)
        .priority(Priority::new(0).unwrap())
        .attempts(AttemptCount::new(1).unwrap())
        .max_attempts(MaxAttempts::new(3).unwrap())
        .scheduled_at(now)
        .claimed_by(WorkerId::new())
        .claimed_until(now)
        .created_at(now)
        .updated_at(now)
        .build()
}
```

**WorkerService builder approach:**

`WorkerService` has 9 fields (6 required, 3 with defaults):

```rust
#[derive(bon::Builder)]
pub struct WorkerService {
    repo: Arc<dyn TaskRepository>,
    registry: Arc<TaskRegistry>,
    config: WorkerConfig,
    queue: QueueName,
    token: CancellationToken,
    worker_id: WorkerId,
    #[builder(default = Arc::new(|_| false))]
    is_saturation: SaturationClassifier,
    #[builder(default)]
    metrics: Option<Metrics>,
    #[builder(default = Arc::new(AtomicU32::new(0)))]
    active_tasks: Arc<AtomicU32>,
}
```

**Consideration:** `bon::Builder` on a struct with `Arc<dyn Trait>` fields works fine — `bon` generates setter methods that accept the field type directly. The builder pattern replaces the existing `with_saturation_classifier()` and `with_metrics()` chainable methods.

**`bon` type-state builder + conditional fields:** `bon` v3 uses type-state builders where each `.field()` call returns a different type (compile-time enforcement that required fields are set). This makes the current conditional pattern (`if let Some(m) = metrics { worker = worker.with_metrics(m) }`) difficult because you can't store the builder in a `let mut` — the type changes. Solutions:
1. **`bon`'s `maybe_` prefix:** For `Option<T>` fields with `#[builder(default)]`, `bon` generates a `.maybe_metrics(Option<Metrics>)` setter. Use `builder.maybe_metrics(self.metrics.clone())` to unconditionally set the Option value.
2. **Always set explicitly:** `.metrics(m.clone())` for `Some`, omit for `None` — but this requires branching the builder chain. Use approach 1 instead.
3. **Manual builder fallback** — if `bon` type-state is too awkward.

Verify `bon`'s `maybe_` prefix support and `Arc<dyn Trait>` compatibility during implementation.

**If `bon::Builder` doesn't work well with `Arc<dyn Trait>` types or type-state is too restrictive** (verify during implementation), fall back to a manual builder:
```rust
pub struct WorkerServiceBuilder { /* fields */ }
impl WorkerServiceBuilder {
    pub fn new(repo: Arc<dyn TaskRepository>, ...) -> Self { ... }
    pub fn saturation_classifier(mut self, f: SaturationClassifier) -> Self { ... }
    pub fn metrics(mut self, m: Metrics) -> Self { ... }
    pub fn build(self) -> WorkerService { ... }
}
```

**`DispatchContext` struct:**

The `dispatch_task` function (lines 382–393) takes 9 parameters. Group them:

```rust
/// Context passed to `dispatch_task` to avoid argument-order bugs.
///
/// All fields are references borrowed from the `WorkerService` for
/// the duration of the dispatch closure.
struct DispatchContext<'a> {
    repo: &'a Arc<dyn TaskRepository>,
    registry: &'a Arc<TaskRegistry>,
    worker_id: WorkerId,
    base_delay_secs: f64,
    max_delay_secs: f64,
    log_payload: bool,
    metrics: Option<&'a Metrics>,
    queue_str: &'a str,
}
```

Then `dispatch_task` becomes:
```rust
async fn dispatch_task(task: &TaskRecord, ctx: &DispatchContext<'_>) { ... }
```

The context is constructed once in `run_poll_loop` before the spawn closure:
```rust
let ctx = DispatchContext {
    repo: &self.repo,
    registry: &self.registry,
    worker_id,
    base_delay_secs: self.config.base_delay.as_secs_f64(),
    max_delay_secs: self.config.max_delay.as_secs_f64(),
    log_payload: self.config.log_payload,
    metrics: self.metrics.as_ref(),
    queue_str: &queue_str,
};
```

**Lifetime consideration:** The `DispatchContext` borrows from `self` (the `WorkerService`). Inside the `join_set.spawn(async move { ... })` closure, the context must be `'static` or cloned. Since `dispatch_task` is called INSIDE the spawned task, the context needs to be owned, not borrowed. Options:

1. **Clone the references:** Make `DispatchContext` own `Arc` clones instead of references:
   ```rust
   struct DispatchContext {
       repo: Arc<dyn TaskRepository>,
       registry: Arc<TaskRegistry>,
       worker_id: WorkerId,
       base_delay_secs: f64,
       max_delay_secs: f64,
       log_payload: bool,
       metrics: Option<Metrics>,  // Metrics is Clone
       queue_str: String,
   }
   ```
   Construct ONCE before the loop, clone into each spawn.

2. **Keep the function signature, just group the struct fields:** Since `dispatch_task` is called inside `join_set.spawn(async move { ... })`, the parameters are already moved/cloned into the closure. The `DispatchContext` just groups them for readability at the call site.

Recommend option 1 — an owned `DispatchContext` that can be `.clone()`d into each spawn. All inner types are cheap to clone (`Arc`, `WorkerId` is `Copy`, primitives, `Metrics` is `Clone`).

**`ActiveTaskGuard` interaction:** The guard at lines 229–234 takes `self.active_tasks.clone()`, `self.metrics.clone()`, `self.config.concurrency`, and `queue_str.clone()`. These overlap with `DispatchContext` fields. Consider either:
- Including `active_tasks` and `concurrency` in `DispatchContext` so the guard can be constructed from the context
- Keeping the guard construction separate (it's only 4 fields, and `active_tasks` is specific to the worker, not the dispatch)

Keeping them separate is simpler — the guard is a worker-level concern, not a dispatch-level concern.

### Previous Story Intelligence

**From Story 6.8 (ready-for-dev):**
- `release_leases_for_worker` SQL changed (attempt increment). No interaction with builder changes.
- `claim_next` to spawn window has a new token check. `DispatchContext` grouping should include this code path.

**From Story 6.6 (ready-for-dev):**
- `TaskError` restructured with new variants. `InvalidPayload` now uses `PayloadErrorKind`. No interaction with builders.

**From Story 6.5 (ready-for-dev):**
- Worker poll loop restructured with backoff. `DispatchContext` is constructed INSIDE the loop — no conflict with backoff state.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide.

### Git Intelligence

Last code commit: `7ed6fc8`. `TaskRecord` constructor last modified in Story 1A.1. `WorkerService::new()` last modified in Story 2.3. `dispatch_task` last modified in Story 3.1.

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `TaskRecord` struct | `crates/domain/src/model/task.rs:77–92` | AC 2 — add `bon::Builder` |
| `TaskRecord::new()` | `crates/domain/src/model/task.rs:108–141` | AC 2 — remove after builder |
| `#[allow(too_many_arguments)]` on new() | `task.rs:108` | AC 2 — remove |
| `WorkerService` struct | `crates/application/src/services/worker.rs:47–57` | AC 3 — add builder |
| `WorkerService::new()` | `worker.rs:66–85` | AC 3 — replace with builder |
| `with_saturation_classifier()` | `worker.rs:95–98` | AC 3 — subsumed by builder |
| `with_metrics()` | `worker.rs:103–106` | AC 3 — subsumed by builder |
| `dispatch_task` | `worker.rs:382–393` | AC 4 — context struct |
| `#[allow(too_many_arguments)]` on dispatch | `worker.rs:382` | AC 4 — remove |
| `IronDeferBuilder` | `crates/api/src/lib.rs:696–720` | Already builder — no change |
| `SweeperService::new()` | `sweeper.rs:40–52` | 3 params — no change needed |
| `deny.toml` | `deny.toml:36–50` | AC 1 — MIT/Apache-2.0 allowed |

### Construction Site Inventory

**TaskRecord (6 sites):**
1. `scheduler.rs:84` — `enqueue` production
2. `scheduler.rs:149` — `enqueue_raw` production
3. `scheduler.rs:232` — `synthetic_record` test
4. `worker.rs:955` — `synthetic_record` test
5. `task_repository_test.rs:34` — `sample_task_with` test
6. `postgres_task_repository.rs:127` — `TryFrom<TaskRow>` adapter

**WorkerService (12 sites):**
1. `lib.rs:430` — production
2. `worker.rs:1030, 1092, 1145, 1178, 1250, 1298, 1400, 1550, 1675, 1773` — 10 test sites

### Dependencies

**New dependency:** `bon` crate
- Add to `[workspace.dependencies]` in root `Cargo.toml`: `bon = "3"` (or latest stable version)
- Add to `crates/domain/Cargo.toml` and `crates/application/Cargo.toml`
- License: MIT/Apache-2.0 — already in `deny.toml` allow-list
- Run `cargo deny check` to verify

### Project Structure Notes

- **Modified files:**
  - `Cargo.toml` (workspace root) — add `bon` dependency
  - `crates/domain/Cargo.toml` — add `bon` dependency
  - `crates/application/Cargo.toml` — add `bon` dependency
  - `crates/domain/src/model/task.rs` — `TaskRecord` builder, remove `new()`
  - `crates/application/src/services/worker.rs` — `WorkerService` builder, `DispatchContext`, tests
  - `crates/application/src/services/scheduler.rs` — update `TaskRecord` construction + test helpers
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs` — update `TryFrom` construction
  - `crates/infrastructure/tests/task_repository_test.rs` — update test helpers
  - `crates/api/src/lib.rs` — update `WorkerService` construction
- **No schema changes, no `.sqlx/` regeneration needed**

### Out of Scope

- **Field visibility / accessor methods** — Story 6.10 (CR46 continued). Fields stay `pub` in this story; 6.10 makes them private with accessors.
- **`IronDeferBuilder` changes** — already a proper builder, no modification needed.
- **`SweeperService` builder** — only 3 params, no change needed.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 479–510] — Story 6.9 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md`] — Enforcement guidelines (no clippy::too_many_arguments)
- [Source: `crates/domain/src/model/task.rs:77–141`] — TaskRecord struct and constructor
- [Source: `crates/application/src/services/worker.rs:47–85`] — WorkerService struct and constructor
- [Source: `crates/application/src/services/worker.rs:382–393`] — dispatch_task signature
- [Source: `deny.toml:36–50`] — License allow-list (MIT and Apache-2.0 allowed)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- `bon` v3 treats `Option<T>` fields as automatically optional — explicit `#[builder(default)]` on `Option` fields is a compile error.
- `bon` type-state builder + closure lifetime: `Arc::new(|_err| ...)` inline in builder `.is_saturation()` causes lifetime inference failure. Fix: bind to a `let classifier: SaturationClassifier` variable first.
- `TaskRecord::new()` removed entirely — all 6 construction sites migrated to builder.
- `WorkerService::new()`, `with_saturation_classifier()`, `with_metrics()` removed — replaced by `#[derive(bon::Builder)]` with `#[builder(default)]` on optional fields.
- `handle_task_failure` parameter count reduced from 10 to 4 via `DispatchContext`.

### Completion Notes List

- Task 1: Added `bon = "3"` to workspace dependencies; domain and application crates reference it. MIT/Apache-2.0 licenses already in deny.toml allow-list.
- Task 2: `TaskRecord` now derives `bon::Builder`. The manual `new()` constructor was removed. All 6 construction sites migrated to the builder. Optional fields (`last_error`, `claimed_by`, `claimed_until`) are automatically optional. Unused `WorkerId` import removed from scheduler.rs.
- Task 3: `WorkerService` now derives `bon::Builder`. `with_saturation_classifier()` and `with_metrics()` chainable methods removed. 13 test call sites + 1 production call site migrated to builder pattern using `maybe_metrics()` for optional metrics.
- Task 4: Owned `DispatchContext` struct created to group 9 `dispatch_task` parameters into 2. `handle_task_failure` also refactored from 10 params to 4. All `#[allow(clippy::too_many_arguments)]` annotations eliminated from the codebase.
- Task 5: Full workspace test suite passes, `cargo clippy --pedantic` clean (no `too_many_arguments`), `cargo fmt --check` clean, `cargo deny check` passes.

### File List

- `Cargo.toml` — added `bon = "3"` to `[workspace.dependencies]`
- `crates/domain/Cargo.toml` — added `bon = { workspace = true }`
- `crates/domain/src/model/task.rs` — added `#[derive(bon::Builder)]` to `TaskRecord`, removed `TaskRecord::new()` constructor
- `crates/application/Cargo.toml` — added `bon = { workspace = true }`
- `crates/application/src/services/scheduler.rs` — migrated 2 production + 1 test `TaskRecord` construction sites to builder; removed unused `WorkerId` import
- `crates/application/src/services/worker.rs` — added `#[derive(bon::Builder)]` to `WorkerService`, removed `new()`/`with_saturation_classifier()`/`with_metrics()`; created `DispatchContext` struct; migrated 13 test + build_privacy_fixture `WorkerService` construction sites; refactored `dispatch_task` and `handle_task_failure` to use `DispatchContext`
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — migrated `TryFrom<TaskRow>` to use `TaskRecord::builder()` with `maybe_` setters for optional fields
- `crates/infrastructure/tests/task_repository_test.rs` — migrated `sample_task_with()` to builder
- `crates/api/src/lib.rs` — migrated `WorkerService` production construction to builder with `maybe_metrics()`

### Review Findings

- [x] [Review][Dismissed] Breaking change for #[non_exhaustive] domain construction — Resolved: User opted to embrace the builder only for construction.
- [x] [Review][Patch] Redundant heap allocations in task dispatch loop — DispatchContext clones self.queue.to_string() for every single task claimed. Also repeated string allocations in metrics labels. [crates/application/src/services/worker.rs:273–285]
- [x] [Review][Patch] Unnecessary payload cloning in dispatch_task — let payload = task.payload.clone(); before spawning the handler. [crates/application/src/services/worker.rs:541]
- [x] [Review][Patch] Repeated string allocations in metrics labels — KeyValue::new("queue", ctx.queue_str.clone()) called multiple times in dispatch_task and handle_task_failure. [crates/application/src/services/worker.rs]
- [x] [Review][Defer] Panic on empty task kind during submission — submit and submit_custom use TaskKind::try_from(kind).expect(...), which will panic if kind is empty. [crates/application/src/services/scheduler.rs:81, 146] — deferred, pre-existing
- [x] [Review][Defer] Lack of builder validation for timestamp invariants — TaskRecord::builder() doesn't ensure updated_at >= created_at. [crates/domain/src/model/task.rs] — deferred, pre-existing
