# Story 8.4: README & Tutorial Examples

Status: done

## Story

As a developer evaluating iron-defer,
I want a clear README and runnable tutorial examples,
so that I can understand what iron-defer does and submit my first durable task within 30 minutes.

## Acceptance Criteria

### AC1: README Rewrite

Given the repository `README.md`,
When a developer reads it,
Then it contains: a one-paragraph summary of what iron-defer is, a feature highlights list (at-least-once, Postgres-only, embedded + standalone, OTel), an inline quick-start code example (5-15 lines showing Task trait + enqueue), and links to guides and examples,
And the README is concise and scannable — under 200 lines,
And no section references files, APIs, or features that don't exist in the current codebase.

### AC2: Tutorial Examples

Given the `crates/api/examples/` directory,
When I list the examples,
Then at minimum these exist as registered `[[example]]` entries in `Cargo.toml`:
- `basic_enqueue.rs` — minimal: define a task, submit, verify completion
- `axum_integration.rs` — embed iron-defer in an axum web server
- `retry_and_backoff.rs` — configure retry limits, observe failure and recovery
- `multi_queue.rs` — multiple named queues with different concurrency
And each example compiles with `cargo check --examples`,
And each example that requires only Postgres can run with `cargo run --example <name>` against a local Postgres (documented in example header comments).

### AC3: 30-Minute Onboarding Path

Given the NFR-U1 target (first durable task within 30 minutes),
When a developer follows the README quick-start → example progression,
Then the path is: read README (2 min) → add `iron-defer` dependency (path or git) → copy `basic_enqueue.rs` pattern → run against Postgres → task completes,
And no step requires reading the architecture document or source code.

## Tasks / Subtasks

- [x] **Task 1: Rewrite README.md** (AC: 1, 3)
  - [x] 1.1: One-paragraph summary with key value props
  - [x] 1.2: 11-item feature highlights list
  - [x] 1.3: Inline quick-start code example (~25 lines showing Task trait + enqueue + start)
  - [x] 1.4: Getting Started section with git dependency + DATABASE_URL + example command
  - [x] 1.5: Examples table, deployment section, configuration, API reference, documentation links
  - [x] 1.6: 143 lines total (under 200)
  - [x] 1.7: All file paths, endpoints, and feature references verified against codebase

- [x] **Task 2: Create `retry_and_backoff.rs` example** (AC: 2)
  - [x] 2.1: Created `crates/api/examples/retry_and_backoff.rs`
  - [x] 2.2: `FlakyTask` fails until attempt N via `ctx.attempt().get()` check
  - [x] 2.3: Uses `engine.enqueue_raw()` with `max_attempts=5`
  - [x] 2.4: Polls status and prints attempt progress
  - [x] 2.5: Task reaches completed after retries
  - [x] 2.6: Header comment with run instructions
  - [x] 2.7: Registered `[[example]]` in Cargo.toml

- [x] **Task 3: Create `multi_queue.rs` example** (AC: 2)
  - [x] 3.1: Created `crates/api/examples/multi_queue.rs`
  - [x] 3.2: `FastTask` and `SlowTask` for two queues
  - [x] 3.3: Two engine instances sharing pool — "fast" (concurrency 4) and "slow" (concurrency 1)
  - [x] 3.4: Enqueues to both, starts both, polls until all complete
  - [x] 3.5: Header comment with run instructions
  - [x] 3.6: Registered `[[example]]` in Cargo.toml

- [x] **Task 4: Update existing examples** (AC: 2)
  - [x] 4.1: `basic_enqueue.rs` — already has header comment with prerequisites and run instructions
  - [x] 4.2: `axum_integration.rs` — already has header comment with prerequisites, run, and test instructions
  - [x] 4.3: All 4 examples compile with `cargo check --examples`

- [x] **Task 5: Verify compilation and onboarding path** (AC: 2, 3)
  - [x] 5.1: `cargo check --examples` passes for all 4 examples
  - [x] 5.2: README → Getting Started → basic_enqueue.rs path is complete and self-contained
  - [x] 5.3: All README references verified (9/9 paths exist)

## Dev Notes

### Current README State

The current `README.md` is 12 lines — just a title, summary line, and two section links (Observability, Compliance Evidence). It needs a complete rewrite to serve as the project's front door for developers.

### Existing Examples

Two examples already exist in `crates/api/examples/`:
- `basic_enqueue.rs` — full task lifecycle: define handler, build engine, enqueue, spawn workers, verify completion. Requires `DATABASE_URL` env var.
- `axum_integration.rs` — embeds engine in custom Axum router, creates `/enqueue` endpoint, demonstrates `State(engine)` passing.

Both are registered as `[[example]]` in `crates/api/Cargo.toml`. The two new examples (`retry_and_backoff.rs`, `multi_queue.rs`) follow the same pattern.

### User-Facing `Task` Trait (NOT `TaskHandler`)

Users implement the `Task` trait from `crates/domain/src/model/task.rs`, NOT `TaskHandler` directly:
```rust
pub trait Task: Send + Sync + Serialize + DeserializeOwned + 'static {
    const KIND: &'static str;
    fn execute(&self, ctx: &TaskContext) -> impl Future<Output = Result<(), TaskError>> + Send;
}
```
- No `#[async_trait]` — uses native `async fn` in trait (Rust 1.75+)
- `Serialize + DeserializeOwned` supertraits are required (payload round-trips through JSON)
- The `KIND` constant is a `&'static str` discriminator (NOT a method)
- `TaskHandler` is an internal object-safe trait; users never implement it

From `crates/api/src/lib.rs`, the canonical usage pattern (see module doc example):
```rust
let engine = IronDefer::builder()
    .pool(pool)
    .register::<EmailTask>()
    .build()
    .await?;
let task = EmailTask { to: "user@example.com".into(), subject: "Hi".into() };
engine.enqueue("default", task).await?;
```

The builder's `.register::<T>()` creates a `TaskHandlerAdapter<T>` internally — users never see this.

### Example Code Structure

All examples follow this structure:
1. Define a struct implementing `Task` (with `Serialize + Deserialize`, `const KIND`, `async fn execute`)
2. Read `DATABASE_URL` from env
3. Create pool via `sqlx::PgPool::connect()`
4. Build engine via `IronDefer::builder().pool(pool).register::<T>().build().await?`
5. Enqueue tasks via `engine.enqueue("queue_name", task_instance).await?`
6. Start engine via `engine.start(token: CancellationToken).await?` (starts workers + sweeper for the configured queue)
7. Wait/poll for completion
8. Cancel token to trigger graceful shutdown

**There is no `start_workers(queue, concurrency)` method.** Workers are started for the queue set via `.queue("name")` on the builder. Concurrency is set via `.worker_config(config)`.

### Retry Configuration

For `retry_and_backoff.rs`, the retry mechanism is:
- `max_attempts` is NOT settable via the typed `engine.enqueue(queue, task)` API (only takes queue + task)
- Use `engine.enqueue_raw(queue, kind, payload, scheduled_at, priority, max_attempts)` to set `max_attempts` — this is the runtime-typed API that takes `serde_json::Value` payload and `Option<i32>` for max_attempts
- Alternatively, the example can use the REST API via `POST /tasks` with `maxAttempts` in the JSON body
- When a handler returns `Err(TaskError::...)`, the task is marked failed and re-queued up to `max_attempts`
- Jittered backoff formula: `base_delay + random(0..base_delay)` with doubling and cap (Architecture §Jittered Backoff)
- The `AttemptCount` and `MaxAttempts` newtypes are in `crates/domain/src/model/attempts.rs`

The handler can inspect the current attempt number via `ctx.attempt()` (singular, returns `AttemptCount`).

### Multi-Queue Configuration

For `multi_queue.rs`:
- Each `IronDefer` engine instance is configured for ONE queue via `.queue("name")` on the builder
- For multiple queues, build MULTIPLE engine instances sharing the same pool, each with a different `.queue()` and `.worker_config()`
- Each engine can have different concurrency via `WorkerConfig`
- Start each engine with `engine.start(token)` using the same or different `CancellationToken`
- Tasks are enqueued to a specific queue via `engine.enqueue("queue_name", task)` — any engine instance can enqueue to any queue

### README Structure Guide

Target structure (under 200 lines):
```
# iron-defer                          (~2 lines)
## Summary paragraph                  (~5 lines)
## Features                           (~15 lines, bullet list)
## Quick Start                        (~30 lines, code example)
## Examples                           (~15 lines, table/list linking to examples/)
## Deployment                         (~10 lines, embedded vs standalone)
## Guides                             (~10 lines, links to docs/guides/)
## Configuration                      (~10 lines, brief + link to config guide)
## Contributing / License             (~10 lines)
```

### Anti-Patterns to Avoid

- **Do NOT write examples that require external services beyond Postgres** — no Redis, no RabbitMQ, no OTel Collector
- **Do NOT write examples that use `unwrap()` on Results** — use `color_eyre` or `anyhow` for error handling (consistent with project style)
- **Do NOT reference docs/guides/ links in README if those guides don't exist yet** — Story 8.5 creates guides. Use placeholder text like "See `docs/guides/` (coming soon)" or better, coordinate with Story 8.5
- **Do NOT add `tokio::time::sleep()` as the primary wait mechanism in NEW examples** — use poll loops or `tokio::signal` for cleaner demos. Note: `basic_enqueue.rs` currently uses `sleep()` — update it to use a poll loop if time permits, but do not break it
- **Do NOT write multi-paragraph doc comments in example files** — header comment with 2-3 lines (what it does, how to run) is sufficient
- **Do NOT invent API surface that doesn't exist** — verify every method call in examples compiles

### Coordination with Story 8.5

Story 8.5 creates the `docs/guides/` directory and guide files. If this story runs first, the README can link to guide paths that will be created by 8.5. If uncertain, use a "Guides" section that lists the planned guides without broken links (e.g., as a feature list rather than hyperlinks).

### Previous Story Intelligence

**From Story 8.1 (done):**
- Architecture fully reconciled — all API names, builder methods, and patterns are current
- `IronDefer::builder()` is the canonical entry point
- `TaskHandlerAdapter<T>` wraps user-defined handlers for type erasure
- Figment 6-step config chain is the canonical configuration approach

**From Epic 7 retrospective:**
- Documentation depth mandate: progressive complexity, every snippet executable
- README → guide → example → test chain must be traceable

### Project Structure Notes

- `README.md` — rewrite (existing, 12 lines → ~100-150 lines)
- `crates/api/examples/retry_and_backoff.rs` — new example
- `crates/api/examples/multi_queue.rs` — new example
- `crates/api/Cargo.toml` — add 2 `[[example]]` entries

### References

- [Source: docs/artifacts/planning/epics.md, Lines 832-860 — Story 8.4 definition, CR54+CR55]
- [Source: crates/api/examples/basic_enqueue.rs — existing example pattern]
- [Source: crates/api/examples/axum_integration.rs — existing example pattern]
- [Source: crates/api/Cargo.toml — [[example]] entries, dependencies]
- [Source: crates/application/src/registry.rs — TaskHandler trait definition]
- [Source: crates/api/src/lib.rs — IronDefer::builder(), register::<T>(), TaskHandlerAdapter]
- [Source: crates/domain/src/model/attempts.rs — AttemptCount, MaxAttempts newtypes]
- [Source: crates/domain/src/model/task.rs — TaskRecord, TaskStatus]
- [Source: crates/api/src/config.rs — figment configuration chain]
- [Source: docs/artifacts/planning/architecture.md §Jittered Backoff — retry formula]
- [Source: docs/artifacts/planning/architecture.md §NFR Coverage — Time-to-first-task < 30min]
- [Source: docs/artifacts/implementation/8-1-architecture-reconciliation-and-engineering-standards.md — previous story]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- `TaskError::handler()` does not exist — use `TaskError::ExecutionFailed { kind: ExecutionErrorKind::HandlerFailed { reason } }` for handler errors
- First engine instance in multi_queue should NOT skip migrations (standalone example needs migrations)

### Completion Notes List

- README.md rewritten from 12 lines to 143 lines (under 200 limit)
- Two new examples: retry_and_backoff.rs (flaky task with retry), multi_queue.rs (dual-queue concurrency)
- All 4 examples compile with `cargo check --examples`
- All README file references verified against codebase (9/9 exist)
- Onboarding path: README → Getting Started → basic_enqueue.rs is self-contained

### Change Log

- 2026-04-24: Implemented all 5 tasks for Story 8.4 — README rewrite, 2 new examples, verification

### File List

- README.md (modified — complete rewrite: summary, features, quick-start, examples, deployment, config, API, docs)
- crates/api/Cargo.toml (modified — added 2 `[[example]]` entries)
- crates/api/examples/retry_and_backoff.rs (new — FlakyTask with retry/backoff demonstration)
- crates/api/examples/multi_queue.rs (new — dual-queue with different concurrency)

### Review Findings

- [x] [Review][Patch] Potential deadlock in multi_queue example during shutdown [crates/api/examples/multi_queue.rs:136]
- [x] [Review][Patch] Concurrent engine migration race in multi_queue example [crates/api/examples/multi_queue.rs:81]
- [x] [Review][Patch] Infinite polling loop in multi-queue example [crates/api/examples/multi_queue.rs:113]
- [x] [Review][Patch] Unguarded task retrieval in retry example [crates/api/examples/retry_and_backoff.rs:86]

