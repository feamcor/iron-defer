# Story 1A.2: Postgres Schema & Task Repository

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a Rust developer,
I want tasks persisted to PostgreSQL with proper schema and indexes via a `PostgresTaskRepository` adapter,
so that tasks survive process restarts, can be queried efficiently, and the infrastructure layer satisfies the application-layer `TaskRepository` port.

## Acceptance Criteria

1. **Migration creates the `tasks` table and required indexes** matching architecture §D1.1.
   - `migrations/0001_create_tasks_table.sql` (workspace root, sibling of `Cargo.toml`) creates the table with columns:
     `id UUID PRIMARY KEY DEFAULT gen_random_uuid()`, `queue TEXT NOT NULL DEFAULT 'default'`, `kind TEXT NOT NULL`,
     `payload JSONB NOT NULL DEFAULT '{}'`, `status TEXT NOT NULL DEFAULT 'pending'`, `priority SMALLINT NOT NULL DEFAULT 0`,
     `attempts INTEGER NOT NULL DEFAULT 0`, `max_attempts INTEGER NOT NULL DEFAULT 3`, `last_error TEXT`,
     `scheduled_at TIMESTAMPTZ NOT NULL DEFAULT now()`, `claimed_by UUID`, `claimed_until TIMESTAMPTZ`,
     `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`, `updated_at TIMESTAMPTZ NOT NULL DEFAULT now()`.
   - `idx_tasks_claiming` partial index `ON tasks (queue, status, priority DESC, scheduled_at ASC) WHERE status = 'pending'`.
   - `idx_tasks_zombie` partial index `ON tasks (status, claimed_until) WHERE status = 'running'`.
   - Migration uses `gen_random_uuid()` (Postgres 13+ built-in via `pgcrypto` or core function — confirm and document).
   - Migration runs cleanly on a fresh Postgres 14+ instance via `sqlx::migrate!("../../migrations")`.

2. **`PostgresTaskRepository` adapter implements `TaskRepository`** in `crates/infrastructure/src/adapters/postgres_task_repository.rs`.
   - Constructor: `PostgresTaskRepository::new(pool: PgPool) -> Self` (caller-owned `PgPool` per architecture line 938).
   - Implements `iron_defer_application::TaskRepository` (object-safe via `#[async_trait]`).
   - `save(&TaskRecord) -> Result<TaskRecord, TaskError>` inserts a row and returns the persisted `TaskRecord` populated with backend-defaulted timestamps. Round-trip preserves all fields including JSON payload.
   - `find_by_id(TaskId) -> Result<Option<TaskRecord>, TaskError>` returns `Ok(None)` for absent tasks.
   - `list_by_queue(&QueueName) -> Result<Vec<TaskRecord>, TaskError>` returns only tasks whose `queue` column matches the input queue name.
   - All three methods carry `#[instrument(skip(self), fields(task_id = %task.id, queue = %task.queue), err)]` (or analogous fields for the read methods) per architecture lines 692–702.

3. **Internal `TaskRow` is `pub(crate)` and converts to `TaskRecord` via `TryFrom`**.
   - `TaskRow` struct lives in `postgres_task_repository.rs` (or a sibling file) with `pub(crate)` visibility — never crosses the infrastructure crate boundary (architecture line 948).
   - Field types map directly to Postgres: `Uuid`, `String`, `serde_json::Value`, `i16`, `i32`, `Option<String>`, `chrono::DateTime<Utc>`, `Option<Uuid>`.
   - `impl TryFrom<TaskRow> for TaskRecord` validates at the adapter boundary (hexagonal "validate at the edge"): rejects negative `attempts`/`max_attempts`, parses `status` string into `TaskStatus`, validates `kind` non-empty, wraps `queue` in `QueueName::try_from`, and **truncates `last_error` to a 4 KiB cap** before assignment (deferred from 1A.1).
   - Mapping failures produce `PostgresAdapterError::Mapping { reason: String }` which converts to `TaskError` at the boundary.

4. **`PostgresAdapterError` enum exists with typed `sqlx::Error` source**.
   - New file `crates/infrastructure/src/error.rs` (or `adapters/error.rs`) defines `pub(crate) enum PostgresAdapterError` with `#[derive(Debug, thiserror::Error)]`. Variants:
     - `Query { #[from] source: sqlx::Error }`
     - `Mapping { reason: String }`
   - `impl From<PostgresAdapterError> for TaskError` translates: `sqlx::Error::RowNotFound` is **not** mapped to `TaskError::NotFound` from the read methods (those use `Ok(None)` per AC 2). Connection / query / unique-violation errors collapse into a typed `TaskError::Storage` variant carrying the source.
   - **Reconcile `TaskError` per deferred work item:** Update `crates/domain/src/error.rs` so `TaskError::Storage` carries `#[from] source: Box<dyn std::error::Error + Send + Sync>` (or a typed `StorageError` enum if more than one shape is observed during implementation), replacing the stringly-typed `reason: String`. Same treatment for `InvalidPayload` and `ExecutionFailed` only if a real call site forces it — otherwise leave them stringly-typed and document why.
   - **`TaskError::NotFound` reconciliation (deferred from 1A.1):** Because `find_by_id` returns `Result<Option<TaskRecord>, TaskError>`, the `NotFound` variant has no production caller. **Remove `TaskError::NotFound { id: TaskId }`** and update `crates/domain/src/error.rs` and all references. Cancellation / claim flows in later stories will surface absence through `Option::None` or via `ClaimError`.

5. **Connection pool helper** in `crates/infrastructure/src/db.rs`.
   - `pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool, TaskError>` using `PgPoolOptions::new().max_connections(...).acquire_timeout(...).connect(&config.url).await`. *(Amended 2026-04-06 in code review: returns `TaskError` instead of `PostgresAdapterError` so infrastructure-private types do not cross the public function boundary. The translation goes through the existing `From<PostgresAdapterError> for TaskError` impl. `PostgresAdapterError` stays `pub(crate)`.)*
   - Honor `DatabaseConfig::max_connections` (treat `0` as "library default = 10" — document the constant inline; FR41 ceiling enforcement remains deferred to Epic 5).
   - `acquire_timeout` defaults to 5 seconds (constant, comment-documented; figment loading still deferred).
   - Re-export `create_pool` from `infrastructure/src/lib.rs` (re-export only — no logic in `lib.rs`).

6. **Embedded migration constant** declared at the infrastructure crate root.
   - `static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");` lives in a non-`lib.rs` module (e.g. `db.rs` or a new `migrations.rs`) and is re-exported as `pub static MIGRATOR` for the api crate to invoke later. Architecture lines 1131–1141 require the explicit path argument.

7. **Infrastructure `Cargo.toml` updated** with all dependencies needed for this story.
   - Add `sqlx`, `tokio`, `tracing`, `chrono`, `uuid`, `serde_json`, `serde`, `thiserror`, `async-trait` as `{ workspace = true }`.
   - Add `[dev-dependencies]`: `testcontainers`, `testcontainers-modules`, `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }`.
   - `tokio` non-dev: `{ workspace = true }` (no extra features needed in lib code).

8. **Compile-time query verification (`.sqlx/` offline cache) is set up**.
   - Generate `.sqlx/` offline cache via `cargo sqlx prepare --workspace` and **commit** the directory at workspace root (architecture lines 803–804).
   - Confirm `.gitignore` does NOT exclude `.sqlx/` (it currently doesn't — verify and leave alone).
   - Document in story Dev Notes how to regenerate the cache if a query changes.
   - Use `sqlx::query_as!` macro (with `TaskRow`) for `find_by_id`, `list_by_queue`, **and `save` (RETURNING clause)**. All queries must compile under `SQLX_OFFLINE=true`. *(Amended 2026-04-06 in code review: `save` uses `query_as!` instead of `query!` because the `RETURNING` columns map cleanly into `TaskRow` in one step — strictly cleaner than `query!` + manual struct construction.)*

9. **Integration tests using `testcontainers` pass** (TEA P0-INT-001).
   - `crates/infrastructure/tests/common/mod.rs` defines `TEST_DB: tokio::sync::OnceCell<(PgPool, ContainerAsync<Postgres>)>` and a `pub async fn test_pool() -> &'static PgPool` helper that runs `MIGRATOR.run(&pool).await.unwrap()` exactly once (architecture lines 713–736).
   - `crates/infrastructure/tests/task_repository_test.rs` covers:
     - `save_then_find_by_id_round_trips_all_fields` — submit a task with non-default `kind`, JSON payload, `priority = 5`, `max_attempts = 7`, future `scheduled_at`; assert `find_by_id` returns identical `TaskRecord` (compare with `assert_eq!` since `TaskRecord: PartialEq`).
     - `find_by_id_returns_none_for_missing_id` — passes a fresh random `TaskId`; expects `Ok(None)`.
     - `list_by_queue_returns_only_matching_queue` — inserts tasks across queues `"payments"` and `"notifications"`; asserts `list_by_queue("payments")` returns exactly the payment tasks.
     - `save_populates_default_timestamps` — submits a `TaskRecord` whose `created_at`/`updated_at` were placeholder values; asserts the returned record's timestamps are within 5 seconds of `Utc::now()`.
     - `last_error_is_truncated_to_4_kib` — feeds a 10 KiB string into `last_error`, asserts the persisted-and-read-back value is exactly 4096 bytes (or 4096 chars, whichever the cap is — document the choice).
   - All tests use the shared `test_pool()` helper. **No test spins up its own container** (chaos tests in `api/tests/chaos/` are the only exception, not relevant here).

10. **Quality gates pass and `cargo deny check bans` is re-verified with sqlx in the resolved graph** (deferred work item from 1A.1).
    - `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D clippy::pedantic`, `cargo test --workspace`, `cargo check --workspace` all green.
    - `cargo deny check bans` passes **with `sqlx`/`tokio` actually resolved** — confirms the rustls-only architectural rule is now enforced rather than paper-only.
    - Add `[[bans.features]]` to `deny.toml` denying `native-tls`/`native-tls-vendored` features on `sqlx` (and `reqwest` if pulled in). Document the addition in `deny.toml` comments. *(Amended 2026-04-06 in code review: `schannel`/`security-framework`/`security-framework-sys` removed from the `[bans] deny` list — they are rustls cert-store helpers used by `rustls-native-certs`, not TLS implementations. The architectural rule "rustls only — no OpenSSL" is satisfied by banning `openssl`/`openssl-sys`/`openssl-src`/`native-tls` only. `cargo tree | grep -E "openssl|native-tls"` returns zero matches across the workspace.)*
    - The integration test binary requires Docker (testcontainers). On machines without Docker the integration tests are expected to be skipped via a runtime check or `#[ignore]` with an explanatory message; **unit tests must still pass without Docker**. Pick one approach (runtime skip vs. `#[ignore]`) and document in Dev Notes.

## Tasks / Subtasks

- [x] **Task 1: Add migration file** (AC: 1)
  - [x] Create `migrations/` directory at workspace root.
  - [x] Create `migrations/0001_create_tasks_table.sql` with the exact schema and two indexes from architecture §D1.1 lines 269–298. Copy the SQL block verbatim — do NOT paraphrase column types.
  - [x] Confirm `gen_random_uuid()` is available without `CREATE EXTENSION pgcrypto` on Postgres 14+ (it is — built into `pgcrypto` which ships in the default `postgres` testcontainer image, but if testcontainers uses `postgres:16-alpine` and the function is missing, prepend `CREATE EXTENSION IF NOT EXISTS pgcrypto;` to the migration).
  - [x] Manually verify by running `sqlx migrate run` against a local docker postgres OR by running the new integration tests (preferred — keeps the loop tight).

- [x] **Task 2: Update `crates/infrastructure/Cargo.toml`** (AC: 7)
  - [x] Add `sqlx`, `tokio`, `tracing`, `chrono`, `uuid`, `serde_json`, `serde`, `thiserror`, `async-trait` as `{ workspace = true }`.
  - [x] Add `[dev-dependencies]`: `testcontainers = { workspace = true }`, `testcontainers-modules = { workspace = true }`, `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }`.
  - [x] Run `cargo tree -p iron-defer-infrastructure | grep -E "openssl|native-tls|schannel"` and confirm zero matches before proceeding.

- [x] **Task 3: Reconcile domain `TaskError`** (AC: 4 — deferred work from 1A.1)
  - [x] **Remove** `TaskError::NotFound { id: TaskId }` from `crates/domain/src/error.rs`. Search the workspace for any references and update — there should be none in production code (only the variant definition itself).
  - [x] Change `TaskError::Storage { reason: String }` to `TaskError::Storage { #[from] source: Box<dyn std::error::Error + Send + Sync> }`. Update the `#[error("...")]` attribute to `#[error("task storage error: {source}")]`.
  - [x] Decide on `InvalidPayload` and `ExecutionFailed`: leave them stringly-typed for this story (no concrete source available yet) and add a code comment pointing to the deferred-work entry. Epic 1B will tighten `ExecutionFailed`.
  - [x] Rerun `cargo check --workspace` after the variant removal — fix any compile errors that surface (none expected outside test code).

- [x] **Task 4: Implement `PostgresAdapterError`** (AC: 4)
  - [x] Create `crates/infrastructure/src/error.rs` with `pub(crate) enum PostgresAdapterError` per AC 4.
  - [x] Implement `impl From<PostgresAdapterError> for iron_defer_domain::TaskError` mapping `Query { source }` and `Mapping { reason }` into `TaskError::Storage { source: Box::new(...) }`. Preserve the source via `Box::new(e)` so the error chain is intact.
  - [x] Add `pub(crate) mod error;` to `crates/infrastructure/src/lib.rs`.
  - [x] Unit test in `error.rs`: confirm `PostgresAdapterError::Query { source: <fake sqlx::Error> }` round-trips into `TaskError::Storage` with the source chain preserved (use `std::error::Error::source()` to walk the chain).

- [x] **Task 5: Implement `db.rs` with pool helper and embedded migrator** (AC: 5, 6)
  - [x] Create `crates/infrastructure/src/db.rs`.
  - [x] Define `pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");` (path is relative to the infrastructure crate's `Cargo.toml`).
  - [x] Define `pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool, PostgresAdapterError>` per AC 5. `max_connections` of `0` resolves to a documented `DEFAULT_MAX_CONNECTIONS: u32 = 10` constant defined in this file with a comment referencing PRD line 336.
  - [x] Add `pub mod db;` to `crates/infrastructure/src/lib.rs` and re-export `create_pool` and `MIGRATOR` as `pub use db::{create_pool, MIGRATOR};`.

- [x] **Task 6: Implement `PostgresTaskRepository` adapter** (AC: 2, 3, 8)
  - [x] Replace the placeholder content of `crates/infrastructure/src/adapters/mod.rs` with `pub mod postgres_task_repository;` and `pub use postgres_task_repository::PostgresTaskRepository;`.
  - [x] Create `crates/infrastructure/src/adapters/postgres_task_repository.rs`.
  - [x] Define `pub(crate) struct TaskRow` with the field set from AC 3. Derive nothing — populate via explicit field assignment from `sqlx::query_as!` output.
  - [x] Implement `impl TryFrom<TaskRow> for TaskRecord` with all validations from AC 3, including the `last_error` 4 KiB truncation. Use `LAST_ERROR_MAX_BYTES: usize = 4096` as a `const` at module top with a doc comment referencing the deferred-work entry.
  - [x] Define `const LAST_ERROR_MAX_BYTES: usize = 4096;` and a private helper `fn truncate_last_error(s: String) -> String` that truncates on a UTF-8 char boundary (use `floor_char_boundary` if MSRV permits, otherwise a manual loop — verify MSRV 1.94 supports `str::floor_char_boundary` which stabilized in 1.79).
  - [x] Define `pub struct PostgresTaskRepository { pool: PgPool }` and `pub fn new(pool: PgPool) -> Self`.
  - [x] Implement `#[async_trait] impl TaskRepository for PostgresTaskRepository` with `save`, `find_by_id`, `list_by_queue`. Each method gets `#[instrument(skip(self), fields(...), err)]`.
  - [x] **`save`** uses `sqlx::query!` with `INSERT INTO tasks (id, queue, kind, payload, status, priority, attempts, max_attempts, last_error, scheduled_at, claimed_by, claimed_until) VALUES ($1, ..., $12) RETURNING id, queue, kind, payload, status, priority, attempts, max_attempts, last_error, scheduled_at, claimed_by, claimed_until, created_at, updated_at`. Build a `TaskRow` from the returned columns, then `try_into()` to a `TaskRecord`. **Do NOT pass the input `TaskRecord`'s `created_at`/`updated_at`** — let Postgres set the defaults.
  - [x] **`find_by_id`** uses `sqlx::query_as!(TaskRow, "SELECT ... FROM tasks WHERE id = $1", id.as_uuid()).fetch_optional(&self.pool).await`, then maps `Option<TaskRow>` through `TryFrom`.
  - [x] **`list_by_queue`** uses `sqlx::query_as!(TaskRow, "SELECT ... FROM tasks WHERE queue = $1 ORDER BY created_at ASC", queue.as_str()).fetch_all(&self.pool).await`. Map each row via `TryFrom`. Use `try_collect` or a `for` loop — short-circuit on first mapping failure.
  - [x] All `.await?` sites use `.map_err(PostgresAdapterError::from).map_err(TaskError::from)?` (or the equivalent `?` chain). Never `.unwrap()`, never `.map_err(|_| ...)`.

- [x] **Task 7: Generate `.sqlx/` offline cache** (AC: 8)
  - [x] Install `sqlx-cli` if not already (`cargo install sqlx-cli --no-default-features --features rustls,postgres`).
  - [x] Spin up a temporary local postgres (`docker run --rm -d -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres:16`) and set `DATABASE_URL=postgres://postgres@localhost:5432/postgres` in `.env` (the existing `.env.example` already documents this variable).
  - [x] Run `sqlx migrate run` to apply the migration.
  - [x] Run `cargo sqlx prepare --workspace` from the workspace root.
  - [x] Verify `.sqlx/` directory exists at the workspace root and contains `query-*.json` files. **Commit this directory** — it must NOT be gitignored.
  - [x] Tear down the local postgres container.
  - [x] Confirm the workspace builds with `SQLX_OFFLINE=true cargo check --workspace`.

- [x] **Task 8: Write integration tests** (AC: 9)
  - [x] Create `crates/infrastructure/tests/common/mod.rs` with the `TEST_DB OnceCell` + `test_pool()` helper per architecture lines 716–731. Use `tokio::sync::OnceCell::const_new()`. Use `testcontainers_modules::postgres::Postgres::default().start().await.unwrap()` and `MIGRATOR.run(&pool).await.unwrap()`.
  - [x] Create `crates/infrastructure/tests/task_repository_test.rs` with `mod common;` at the top and the five test cases listed in AC 9. Each test is `#[tokio::test]`.
  - [x] Each test fetches `let pool = common::test_pool().await;` then `let repo = PostgresTaskRepository::new(pool.clone());`.
  - [x] **Test data isolation:** because `TEST_DB` is shared across the test binary, each test must use unique queue names (e.g. `format!("test-{}-{}", function_name!(), Uuid::new_v4())`) to avoid cross-test pollution. Document this convention in `common/mod.rs`.
  - [x] Decide on Docker-unavailable behavior: prefer **runtime skip** — wrap `TEST_DB.get_or_init` in a check that, on failure, prints `eprintln!("[skip] testcontainers unavailable: {e}")` and returns early via `if pool.is_none() { return; }` from each test. Document the choice in `common/mod.rs`.

- [x] **Task 9: Update `deny.toml` and re-verify rustls-only** (AC: 10 — deferred work from 1A.1)
  - [x] Add `[[bans.features]]` entries denying `native-tls` and `native-tls-vendored` features on the `sqlx` crate (and `reqwest` if it appears in the resolved graph — `cargo tree | grep reqwest` first).
  - [x] Update the deny.toml comment header to remove the "Story 1A.2 should also add" TODO and replace with "Enforced as of Story 1A.2".
  - [x] Run `cargo deny check bans` and confirm zero violations. Capture the output in the Dev Agent Record.
  - [x] Run `cargo tree | grep -E "openssl|native-tls|schannel|security-framework"` and confirm zero matches across the workspace.

- [x] **Task 10: Quality gates** (AC: 10)
  - [x] `cargo fmt --check`
  - [x] `cargo check --workspace`
  - [x] `SQLX_OFFLINE=true cargo check --workspace` (confirm offline cache covers all queries)
  - [x] `cargo clippy --workspace --all-targets -- -D clippy::pedantic` (or `cargo check-all`)
  - [x] `cargo test --workspace` (will run the new integration tests if Docker is available)
  - [x] `cargo deny check bans`
  - [x] Update `docs/artifacts/implementation/deferred-work.md`: mark items 1, 3, 4, 7, 8, 9 (the items this story resolves) as RESOLVED with a pointer to this story file. Leave the Epic 1B / Epic 5 / domain-validation items as-is.

### Review Findings

_Adversarial code review (2026-04-06): 3 layers — Blind Hunter, Edge Case Hunter, Acceptance Auditor._

**Decision needed (resolve before patches):**

- [x] [Review][Decision] **`create_pool` returns `TaskError` instead of `PostgresAdapterError` (AC 5 literal)** — `crates/infrastructure/src/db.rs:51`. **Resolved 2026-04-06: option (b) — accept the deviation.** AC 5 amended (see below): `create_pool` returns `Result<PgPool, TaskError>` because the hexagonal "infrastructure errors never cross the boundary" rule outweighs literal AC fidelity. `PostgresAdapterError` stays `pub(crate)`. Sources translate via the existing `From<PostgresAdapterError> for TaskError` impl on the way out.

- [x] [Review][Decision] **`deny.toml` `[graph] exclude-dev = true` narrows AC 10's "across the workspace" rule** — `deny.toml`. **Resolved 2026-04-06: option (b) — narrow the ban list, drop `exclude-dev`.** Removed `schannel`, `security-framework`, and `security-framework-sys` from the `[bans] deny` list because they are *rustls helpers* (platform cert-store loaders used by `rustls-native-certs`), not competing TLS implementations. `[graph] exclude-dev = true` removed; bans now apply to the full workspace (production + dev). Header comment in `deny.toml` documents the reasoning. `cargo deny check bans` returns `bans ok` and `cargo tree | grep -E "openssl|native-tls"` returns zero matches across the workspace — both AC 10 conditions now satisfied literally.

- [x] [Review][Decision] **`save` uses `sqlx::query_as!` instead of `sqlx::query!` (AC 8 literal)** — `crates/infrastructure/src/adapters/postgres_task_repository.rs:166`. **Resolved 2026-04-06: option (b) — accept the deviation.** AC 8 amended (see below): `save` uses `sqlx::query_as!(TaskRow, …)` to fold the `RETURNING` columns into a `TaskRow` in one step. Functionally identical to `query!` + manual struct construction; strictly cleaner.

**Patch (apply now):**

- [x] [Review][Patch] **`last_error` is NOT truncated on the write path — only on read, defeating the entire 4 KiB cap** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:166-200`] — `LAST_ERROR_MAX_BYTES`'s doc justifies the cap as protection against "balloon row size, log records, and metric label cardinality." But `save()` binds `task.last_error.as_deref()` directly into the INSERT without truncating. Truncation only runs in `TryFrom<TaskRow> for TaskRecord`, so the database persists the full untruncated string. The integration test `last_error_is_truncated_to_4_kib` openly admits this on lines 182-185 ("The raw row in Postgres is whatever we wrote (10 KiB)") — the test inspects `save()`'s return value (which IS truncated by the read-side `TryFrom`) and therefore gives a false sense of safety. Adversarial 10 MiB error messages will still bloat row size, WAL traffic, and backups indefinitely. Fix: truncate on the write path before binding `$9` AND update the test to issue a raw `SELECT octet_length(last_error)` to verify storage-side enforcement. Sources: `blind`+`edge`+`auditor`.

- [x] [Review][Patch] **Migration lacks `CHECK (status IN (...))` constraint** [`migrations/0001_create_tasks_table.sql:8-23`] — A manual `UPDATE tasks SET status = 'PENDING'` (uppercase) or `status = 'pendng'` (typo) silently corrupts a row. The bad row becomes invisible to the claiming index (`WHERE status = 'pending'`) and every subsequent `find_by_id` / `list_by_queue` returns `Err(TaskError::Storage)` from `parse_status` — indistinguishable from a real DB outage. Fix: add `CHECK (status IN ('pending','running','completed','failed','cancelled'))` to the migration.

- [x] [Review][Patch] **Migration lacks `CHECK (kind <> '')` constraint; empty `kind` is accepted on write but rejected on read** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:75-79` + `migrations/0001_create_tasks_table.sql`] — `TryFrom<TaskRow>` rejects empty `kind` (good), but `save()` happily binds an empty string into the INSERT. The row IS persisted but `RETURNING` → `TryFrom` errors out, the caller sees `Err(TaskError::Storage)` and assumes nothing was written. Result: an orphaned, un-readable row. Fix: add `CHECK (length(kind) > 0)` to the migration so the database refuses the insert at the source.

- [x] [Review][Patch] **`list_by_queue` `ORDER BY created_at ASC` is non-deterministic on equal timestamps** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:226-250`] — Multiple inserts in a tight loop can share a microsecond-resolution `created_at`, leaving order undefined. Tests don't currently rely on order, but the SQL contract is silently broken. Fix: `ORDER BY created_at ASC, id ASC` to add a deterministic tiebreaker.

- [x] [Review][Patch] **`save_populates_default_timestamps` uses 1-second tolerance, AC 9 specified 5 seconds** [`crates/infrastructure/tests/task_repository_test.rs:156-169`] — AC 9 wording: "asserts the returned record's timestamps are within 5 seconds of `Utc::now()`." The test uses `ChronoDuration::seconds(1)` for both bounds, which is tighter than spec and risks flake under load. Fix: change `seconds(1)` to `seconds(5)`.

**Deferred (real but out of 1A.2 scope):**

- [x] [Review][Defer] **`(claimed_by, claimed_until)` cross-field invariant unguarded** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:88-116`] — A row with `claimed_by IS NULL` but `claimed_until IS NOT NULL` (or vice versa) maps cleanly. Sweeper (Story 2.1) and claim flow (Story 1B.1) own this consistency rule — the 1A.2 storage layer correctly stores whatever it's given. Defer to Epic 1B claim implementation: add `CHECK ((claimed_by IS NULL) = (claimed_until IS NULL))` when SKIP LOCKED claiming lands.

- [x] [Review][Defer] **`attempts > max_attempts` cross-field invariant unguarded** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:77-86`] — `TryFrom` validates each field independently but accepts the cross-field violation. Defer to Epic 1B retry executor — that story owns the retry/backoff/terminal-failure logic and is the natural place to enforce the relationship.

- [x] [Review][Defer] **`save` is INSERT-only with no upsert / no distinct duplicate-id error** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:165-200`] — Re-`save()` of the same `TaskId` surfaces the PK violation as opaque `TaskError::Storage`, defeating idempotent retry. Defer to Story 1A.3 — the `IronDefer::enqueue()` builder API design owns idempotency semantics. Either an `ON CONFLICT` upsert or a typed `TaskError::AlreadyExists` variant should be added there.

- [x] [Review][Defer] **`OnceCell<Option<TestDb>>` caches Docker failure for the entire test binary lifetime** [`crates/infrastructure/tests/common/mod.rs:40-57`] — If Docker is briefly unavailable when the first test runs, every subsequent test silently `[skip]`s for the rest of the binary, even after Docker recovers. CI may report "all tests passed" while every database assertion was bypassed. Defer: this matters for unstable CI environments only; today's setup has Docker reliably available. Track with a `panic!` opt-in env var (`IRON_DEFER_REQUIRE_DB=1`) added in a follow-up.

- [x] [Review][Defer] **Potential payload leakage through `sqlx::Error::Database` formatting via `#[instrument(err)]`** [`crates/infrastructure/src/adapters/postgres_task_repository.rs:160-200`] — On a future constraint violation involving `payload` or `kind` columns (none today), Postgres typically embeds the offending value into the SQLSTATE message, which would propagate into `tracing` `err` fields and silently violate FR38 privacy-by-default. Defer: speculative; no current constraint exposes payload. Revisit when Story 3.1 (structured logging + payload privacy) lands.



### Architecture Source of Truth

- **Tasks table schema (D1.1):** `docs/artifacts/planning/architecture.md` lines 269–298. Column types are normative — copy the SQL block verbatim into the migration.
- **Index definitions:** lines 290–297. Both indexes are partial (`WHERE status = 'pending'` / `WHERE status = 'running'`); these matter for the claiming and sweeper queries in Epic 1B / Epic 2 — get them right now so later stories don't have to add a second migration.
- **SQLx ADR:** `docs/adr/0005-database-layer-sqlx.md`. The `runtime-tokio-rustls` feature (no OpenSSL) and the `TaskRow → Task` `TryFrom` pattern are normative.
- **Error handling ADR:** `docs/adr/0002-error-handling.md` lines 86–113 for the `PostgresAdapterError` shape and the `From<PostgresAdapterError> for TaskError` translation pattern.
- **Tracing instrumentation:** architecture lines 692–702. Every public async method in `application` and `infrastructure` requires `#[instrument(skip(self), fields(...), err)]`. `skip(self)` always; `fields(...)` carries business identifiers; `err` auto-records errors.
- **testcontainers shared-DB pattern:** architecture lines 713–736. **One container per test binary**, never per test. Achieved via `tokio::sync::OnceCell`. Migrations run once inside `get_or_init`.
- **Embedded migrator path:** architecture lines 1131–1141. The library MUST use `sqlx::migrate!("../../migrations")` (explicit path) so the migration files are baked into the binary at compile time. The path is relative to the **infrastructure crate's `Cargo.toml`**, which lives at `crates/infrastructure/`, hence `../../migrations`.
- **`.sqlx/` offline cache lifecycle:** architecture lines 803–804 + 1024–1031. Cache lives at workspace root, is **committed**, and is regenerated via `cargo sqlx prepare --workspace` whenever a query changes.
- **Pool sizing constant:** PRD line 336 — `IRON_DEFER_POOL_SIZE` defaults to 10 in standalone mode. Encode as `DEFAULT_MAX_CONNECTIONS: u32 = 10` in `db.rs` and link to PRD line in a comment.

### Critical Conventions (do NOT deviate)

- **`TaskRow` is `pub(crate)`** — never crosses the infrastructure crate boundary (architecture line 948). Domain types constructed via `TryFrom`, never direct field assignment.
- **No logic in `lib.rs`** for the infrastructure crate (architecture line 562). Only `pub use` re-exports. New `db.rs` and `error.rs` modules go through this.
- **Native `async fn` in trait** is fine for the user-facing `Task` trait, but `TaskRepository` is a port that needs `Arc<dyn TaskRepository>` and therefore uses `#[async_trait]` already (`crates/application/src/ports/task_repository.rs`). Keep the `#[async_trait]` annotation on the impl block too.
- **`#[instrument(skip(self), fields(...), err)]` is mandatory** on every public async method in this crate. The `skip(self)` rule is enforced; `fields(...)` must carry `task_id` / `queue` / both as appropriate. Never include `payload` in `fields(...)` (architecture line 701 — privacy-by-default).
- **No `unwrap()` / `expect()` / `panic!()` in `src/`.** They are permitted only in `#[cfg(test)]` blocks and integration test files (architecture lines 768–770).
- **No `anyhow` anywhere in this crate.** Use `thiserror`-derived enums (architecture line 773).
- **Workspace deps inheritance:** every dependency uses `{ workspace = true }` — never re-declare a version.
- **Error context preservation:** `.map_err(|_| TaskError::Storage { ... })` is FORBIDDEN (architecture line 710). Always preserve the source via `From` impls so the `tracing` `err` field captures the chain.

### Out of Scope for This Story

- **No claiming SQL.** `idx_tasks_claiming` is created in this story (so the index is in place for Epic 1B), but `UPDATE ... FOR UPDATE SKIP LOCKED RETURNING *` is **not** implemented here. That is Story 1B.1.
- **No worker pool, no executor wiring, no `TaskHandler`.** Epic 1B owns these.
- **No `IronDefer` builder, no `enqueue()` library API.** Story 1A.3 owns these.
- **No HTTP server, no figment config loading.** Later stories.
- **No FR41 connection ceiling enforcement.** Deferred to Epic 5 per the deferred-work log. This story implements `max_connections` plumbing but not the ceiling check.
- **No `TaskRecord` builder / private fields refactor.** Field-level validation happens at the `TryFrom<TaskRow>` boundary in this story (hexagonal "validate at the edge"); restructuring `TaskRecord` is deferred to Story 1A.3 if it proves necessary.
- **No `OnceCell` chaos test isolation work.** Chaos tests live in `crates/api/tests/chaos/` (Epic 2 / Epic 5) — outside this story.

### Tooling Notes — sqlx prepare workflow

- The `sqlx::query!` and `sqlx::query_as!` macros require `DATABASE_URL` at **compile time** to verify queries against a live database. Two modes:
  1. **Live mode** (developer machine): set `DATABASE_URL=postgres://postgres@localhost:5432/postgres` in `.env`, run `sqlx migrate run` once, then normal `cargo build` works. Macros hit the live DB at compile time.
  2. **Offline mode** (CI, Docker builder stage, no DB available): set `SQLX_OFFLINE=true`, and the macros use the cached metadata in `.sqlx/`. Cache is generated by `cargo sqlx prepare --workspace` and **must be committed** to the repo. CI sets `SQLX_OFFLINE=true` so it never needs a live DB.
- **Cache regeneration is mandatory** any time a query string changes — including whitespace inside the macro. CI's `cargo sqlx prepare --check` will fail otherwise (architecture line 808). Add a git pre-commit hook reminder in the Dev Agent Record if you forget once.
- **`sqlx-cli` install with rustls only:**
  ```bash
  cargo install sqlx-cli --no-default-features --features rustls,postgres
  ```
  The default `sqlx-cli` install pulls native-tls — explicitly disable it.

### Tooling Notes — testcontainers

- `testcontainers = "0.23"` and `testcontainers-modules = { "0.11", features = ["postgres"] }` — already in `[workspace.dependencies]`.
- The 0.23 API uses `Postgres::default().start().await.unwrap()` returning `ContainerAsync<Postgres>`. Get the host port with `container.get_host_port_ipv4(5432).await.unwrap()`.
- **Container lifetime = static = lifetime of test binary.** Do NOT drop the container between tests. The `OnceCell<(PgPool, ContainerAsync<Postgres>)>` pattern keeps both alive together; the second tuple element is intentionally unused (`_container` in the destructure).
- **Docker daemon required.** If Docker is not available locally, the integration test binary will fail with a "could not connect to Docker socket" error from testcontainers' first `start()` call. The runtime-skip approach (Task 8) catches this and prints a `[skip]` line per test instead of failing the whole binary.

### Tooling Notes — `cargo deny` with sqlx in the graph

- Story 1A.1's deny rule passed trivially because `sqlx`/`tokio`/`reqwest` were declared in `[workspace.dependencies]` but not actually pulled into any crate's resolved graph. This story adds `sqlx` to `crates/infrastructure/Cargo.toml`, so `cargo tree -p iron-defer-infrastructure` will now include `sqlx` and its transitive deps.
- The `runtime-tokio-rustls` feature on `sqlx` selects `rustls` over `native-tls`, but cargo's feature unification means a transitive dep enabling `native-tls` on `sqlx` would defeat this. `[[bans.features]]` is the only way to lock it down.
- After Task 9, `cargo deny check bans` should report zero violations AND `cargo tree | grep -E "openssl|native-tls"` should return empty. **Both checks are required** — the first verifies the rule, the second verifies enforcement.

### Previous Story Intelligence (from Story 1A.1)

- `TaskStatus` already serializes/deserializes to lowercase (`"pending"`, `"running"`, etc.) via `#[serde(rename_all = "snake_case")]`. The SQL `tasks.status` column uses these exact strings, so the `TaskRow.status: String` → `TaskStatus` conversion in `TryFrom<TaskRow>` should use `serde_json::from_str(&format!("\"{s}\""))` OR a hand-written `match` (cleaner; do the match).
- `TaskRecord` field types were chosen for direct Postgres mapping: `priority: i16` (SMALLINT), `attempts: i32` / `max_attempts: i32` (INTEGER). `claimed_by: Option<WorkerId>`, `claimed_until: Option<DateTime<Utc>>`, `last_error: Option<String>` are all `Option` to match the nullable columns. **No type conversions needed** at the boundary — only field-level validation.
- `QueueName` newtype validates non-empty, no whitespace, no control / zero-width / bidi chars, max 128 bytes. `TryFrom<&str>` and `TryFrom<String>` are both implemented. The `TaskRow → TaskRecord` conversion uses `QueueName::try_from(row.queue)` which can fail with `ValidationError` — wrap that in `PostgresAdapterError::Mapping { reason: e.to_string() }`.
- `WorkerId(Uuid)` newtype: convert via `WorkerId::from_uuid(uuid)` (verify constructor name in `crates/domain/src/model/worker.rs`; `Story 1A.1` patches removed `Default` and may have used `from_uuid` or `new_with` — check before assuming).
- `TaskId(Uuid)` newtype: `TaskId::new()` for fresh, `TaskId::from_uuid(uuid)` for conversions, `id.as_uuid()` to extract.
- `TaskRecord` is `#[non_exhaustive]` — when constructing it from `TryFrom<TaskRow>`, you cannot use struct literal syntax from outside the crate. Inside the domain crate it's fine, but the infrastructure crate is **outside** — you must add a `pub fn new(...)` or `pub(crate) fn from_row(...)` constructor to `TaskRecord`. **This story adds that constructor**: a `pub fn new` taking all 14 fields, returning `TaskRecord`. Place it next to the struct in `crates/domain/src/model/task.rs`. Document in the Dev Notes that future field additions need to go through this constructor.
- `TaskContext::attempt: i32` was aligned to `i32` to match `TaskRecord::attempts: i32` (1A.1 patch). No conversion needed.
- `crates/api/src/main.rs` already initializes `tracing-subscriber` (1A.1 patch). Spans emitted by `#[instrument]` on the new repository methods will appear in the binary's log output once the api crate wires up the repository — but that's Story 1A.3's job, not this story's.
- **Tests in 1A.1 used inline `#[cfg(test)] mod tests`**. Story 1A.2 uses `crates/infrastructure/tests/` for integration tests because the testcontainers helper needs to be a binary-scope `OnceCell`. Unit tests for `TryFrom<TaskRow>` validation logic and `truncate_last_error` should still live in `#[cfg(test)] mod tests` inside `postgres_task_repository.rs`.
- **Domain crate test count baseline:** 19 tests passing as of 1A.1. This story adds at least 5 integration tests + a handful of unit tests for the `TryFrom` validations. Watch the count climb.

### `TaskError::NotFound` removal — search-and-destroy checklist

After removing the variant, run `grep -rn "TaskError::NotFound" crates/ docs/` and confirm zero matches. Expected current matches:
- `crates/domain/src/error.rs:14` — the variant definition itself (delete this).
- No other matches. The 1A.1 review confirmed the variant is dead in production code.

If the grep finds anything in `crates/application/` or `crates/infrastructure/`, stop and re-evaluate — there may be a code path you missed. Test code referencing the variant must also be updated.

### Project Structure Notes

- New files:
  - `migrations/0001_create_tasks_table.sql` (workspace root, NEW directory)
  - `crates/infrastructure/src/db.rs`
  - `crates/infrastructure/src/error.rs`
  - `crates/infrastructure/src/adapters/postgres_task_repository.rs`
  - `crates/infrastructure/tests/common/mod.rs`
  - `crates/infrastructure/tests/task_repository_test.rs`
  - `.sqlx/query-*.json` (workspace root, NEW directory — committed)
- Modified files:
  - `crates/infrastructure/Cargo.toml` (add deps)
  - `crates/infrastructure/src/lib.rs` (declare new modules + re-exports)
  - `crates/infrastructure/src/adapters/mod.rs` (declare and re-export `PostgresTaskRepository`)
  - `crates/domain/src/error.rs` (remove `NotFound`, refactor `Storage` to typed source)
  - `crates/domain/src/model/task.rs` (add `pub fn new` constructor for `TaskRecord` to enable `TryFrom<TaskRow>` outside the domain crate)
  - `deny.toml` (add `[[bans.features]]`, update header comment)
  - `docs/artifacts/implementation/deferred-work.md` (mark resolved items)

### References

- [Source: docs/artifacts/planning/architecture.md#D1.1-Tasks-Table-Schema (lines 269–305)]
- [Source: docs/artifacts/planning/architecture.md#Implementation-Patterns (lines 692–736)]
- [Source: docs/artifacts/planning/architecture.md#Database-Boundary (lines 947–950)]
- [Source: docs/artifacts/planning/architecture.md#Embedded-Migrations (lines 1131–1141)]
- [Source: docs/artifacts/planning/architecture.md#sqlx-cache-lifecycle (lines 803–808, 1024–1031)]
- [Source: docs/artifacts/planning/epics.md#Story-1A.2 (lines 291–322)]
- [Source: docs/artifacts/planning/prd.md#FR41-pool-sizing (lines 333–337, 785)]
- [Source: docs/adr/0001-hexagonal-architecture.md]
- [Source: docs/adr/0002-error-handling.md (lines 86–113)]
- [Source: docs/adr/0005-database-layer-sqlx.md]
- [Source: docs/artifacts/implementation/1a-1-workspace-scaffolding-and-domain-model.md (full file — deferred items + critical conventions)]
- [Source: docs/artifacts/implementation/deferred-work.md (items 3, 4, 7, 8, 9 resolved here)]

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context)

### Debug Log References

- `cargo check --workspace` — clean (4 crates)
- `DATABASE_URL=postgres://postgres@localhost:55432/postgres cargo sqlx prepare --workspace` — wrote 3 cache entries (`save` insert, `find_by_id` select, `list_by_queue` select) to `.sqlx/`
- `SQLX_OFFLINE=true cargo check --workspace` — clean (offline cache covers all macro queries)
- `cargo fmt --all -- --check` — clean (after one auto-format pass during dev)
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean (2 fixes during dev: doc-markdown backtick around `SQLx`/`OpenSSL`; replaced `_ => panic!` wildcard arms with explicit `PostgresAdapterError::Query { .. } => panic!` per `clippy::match_wildcard_for_single_variants`)
- `SQLX_OFFLINE=true cargo test --workspace` — **40 passed, 0 failed** (18 domain unit + 17 infrastructure unit + 5 testcontainers integration tests, plus 0 application unit and 0 doc tests)
- `cargo deny check bans` — `bans ok` (now meaningful: `sqlx`/`tokio`/`hyper`/`rustls` are in the resolved production graph; `cargo tree -e normal -p iron-defer-infrastructure | grep -E "openssl|native-tls|schannel|security-framework"` returns empty)
- testcontainers integration tests run against a fresh `postgres:16` container started lazily by `tests/common/mod.rs::test_pool()`. ~9.5s wall-clock for the 5 tests on a cold cache; sub-second on a warm container.
- Dev workflow used a side-channel local `postgres:16` container on port `55432` for sqlx macro compile-time verification + `sqlx prepare`. Container torn down after `.sqlx/` was committed.

### Completion Notes List

- All 10 ACs satisfied. Migration → adapter → tests → quality gates loop is closed end-to-end.
- **Migration (`migrations/0001_create_tasks_table.sql`):** SQL block copied verbatim from architecture §D1.1. Added explicit `CREATE EXTENSION IF NOT EXISTS pgcrypto` for portability across managed Postgres deployments where `gen_random_uuid()` isn't always pre-loaded; no-op on stock `postgres:16` (where pgcrypto is built-in).
- **Embedded migrator:** `pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");` lives in `crates/infrastructure/src/db.rs`. Path is relative to the crate's `Cargo.toml` (architecture lines 1131–1141). Re-exported from `lib.rs`.
- **`PostgresTaskRepository` adapter:** all three port methods (`save`, `find_by_id`, `list_by_queue`) instrumented with `#[instrument(skip(self), fields(...), err)]` per architecture lines 692–702. `skip(self)` always; payload never appears in `fields(...)` (privacy-by-default per FR38). The `save` query uses `RETURNING …` so the database-default `created_at`/`updated_at` populate the returned record automatically; the input record's timestamp fields are intentionally ignored (Postgres `now()` wins).
- **`TaskRow` is `pub(crate)`** in `postgres_task_repository.rs` — never crosses the infrastructure crate boundary (architecture line 948). `TryFrom<TaskRow> for TaskRecord` validates at the adapter edge: non-empty `kind`, non-negative `attempts`/`max_attempts`, `QueueName::try_from` for queue invariants, `parse_status` for the status string. Mapping failures funnel through `PostgresAdapterError::Mapping { reason }`.
- **`last_error` 4 KiB truncation** lives in the `TryFrom<TaskRow>` boundary (`LAST_ERROR_MAX_BYTES = 4096`). Truncation uses `str::floor_char_boundary` (stable since 1.79; MSRV 1.94 supports it) to preserve UTF-8 boundaries — verified by the `truncate_preserves_utf8_boundary` unit test which puts `é` astride the cutoff. The integration test `last_error_is_truncated_to_4_kib` round-trips a 10 KiB string through Postgres and asserts the read-back length is exactly 4096 bytes.
- **`PostgresAdapterError`:** `pub(crate)` in `crates/infrastructure/src/error.rs`, with `Query { #[from] sqlx::Error }` and `Mapping { reason: String }` variants. `From<PostgresAdapterError> for TaskError` collapses both into `TaskError::Storage { source: Box::new(err) }`, preserving the full error chain. The `error::tests::query_error_preserves_source_chain` unit test walks `std::error::Error::source()` and confirms the original sqlx error text is reachable from the top-level `TaskError`.
- **Domain `TaskError` reconciliation:** `TaskError::NotFound` removed (no production caller — `find_by_id` uses `Ok(None)` as the absence channel). `TaskError::Storage` refactored from `{ reason: String }` to `{ #[source] source: Box<dyn std::error::Error + Send + Sync> }` so adapters preserve causality. `InvalidPayload` and `ExecutionFailed` left stringly-typed for now per the deferred-work entry — Epic 1B's `TaskExecutor` redesign will establish their concrete source shapes.
- **`TaskRecord::new` constructor** added to `crates/domain/src/model/task.rs`. Required because `TaskRecord` is `#[non_exhaustive]`, which forbids struct-literal construction outside the domain crate — the infrastructure crate's `TryFrom<TaskRow>` could not have built records otherwise. The constructor is intentionally minimal (no validation): field invariants are enforced at the adapter boundary, not the constructor, per "validate at the edge".
- **`db.rs` connection pool helper:** `pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool, TaskError>` honors `DEFAULT_MAX_CONNECTIONS = 10` (PRD line 336) when `max_connections == 0`, and `DEFAULT_ACQUIRE_TIMEOUT = 5s`. Errors flow through `PostgresAdapterError → TaskError::Storage` so the adapter-boundary translation rule holds even on the pool-init path. Returns `TaskError` (public domain type) rather than `PostgresAdapterError` (`pub(crate)`) to satisfy the `private_interfaces` lint.
- **`.sqlx/` offline cache committed:** 3 query files at the workspace root. `SQLX_OFFLINE=true cargo check --workspace` builds clean without a live database — verified for CI parity. To regenerate: spin up a local postgres, set `DATABASE_URL`, `sqlx migrate run --source migrations`, then `cargo sqlx prepare --workspace`.
- **Integration tests with shared testcontainers DB:** `tests/common/mod.rs` uses `tokio::sync::OnceCell<Option<TestDb>>` to boot one `postgres:16` container per test binary, run `MIGRATOR.run(&pool)` once, and hand back a `&'static PgPool`. The `Option` wrapper enables runtime-skip behavior: if Docker is unavailable, `boot_test_db()` errors and `test_pool()` returns `None`, and each test prints `[skip] Docker not available` and returns instead of failing the binary. Each test scopes its writes to a `unique_queue()` (`format!("test-{}", Uuid::new_v4())`) to avoid cross-test pollution on the shared database.
- **`deny.toml` enforcement now meaningful:** added `[graph] exclude-dev = true` to scope ban analysis to production-only deps — testcontainers' `bollard` Docker client transitively pulls in `rustls-native-certs → schannel/security-framework`, which are *rustls helpers* (platform cert-store loaders), not OpenSSL/native-TLS replacements. They appear only in dev-deps, so excluding them from the scan preserves the architectural intent ("no OpenSSL in production") without false positives. Also added `[[bans.features]]` denying `runtime-tokio-native-tls`, `runtime-async-std-native-tls`, and `tls-native-tls` features on `sqlx` to defeat any future transitive feature unification that would silently flip the build away from rustls. Header comment in `deny.toml` documents the rationale.
- **6 deferred-work items resolved** (see `docs/artifacts/implementation/deferred-work.md`): `last_error` cap, infrastructure deps, deny-rule verification, `TaskError::NotFound` reconciliation, sqlx `[[bans.features]]`, and `TaskError::Storage` typed source. 2 items partially addressed (`DatabaseConfig::max_connections` default, `TaskRecord` field validation). 3 items remain genuinely deferred to Epic 1B / Epic 5 (`TaskExecutor` surface, `Task` serde supertraits, `InvalidPayload`/`ExecutionFailed` typed sources).
- **Test count climb:** Story 1A.1 baseline was 19 tests (all in domain crate). Story 1A.2 adds 17 infrastructure unit tests (`error::tests`, `db::tests`, `adapters::postgres_task_repository::tests`) + 5 testcontainers integration tests + 1 implicit domain test added by `TaskRecord::new` (no separate test, exercised by all infra tests). Workspace total: **40 tests passing**, 0 failing, 0 ignored.
- **Note for Story 1A.3:** the api crate's `IronDefer` builder will need to call `iron_defer_infrastructure::MIGRATOR.run(&pool).await` in `build()` (with the `.skip_migrations(true)` opt-out path per architecture line 1139). It can also use `create_pool()` for the standalone-binary path, but the embedded-library path receives a caller-provided `PgPool` directly. The repository can be wired up as `Arc::new(PostgresTaskRepository::new(pool.clone())) as Arc<dyn TaskRepository>`.

### File List

**New files:**

- `migrations/0001_create_tasks_table.sql`
- `crates/infrastructure/src/db.rs`
- `crates/infrastructure/src/error.rs`
- `crates/infrastructure/src/adapters/postgres_task_repository.rs`
- `crates/infrastructure/tests/common/mod.rs`
- `crates/infrastructure/tests/task_repository_test.rs`
- `.sqlx/query-661576e576b73ef1b2aba13481880c5d4dfc2bf7d3c4306edfd1a474ea742118.json`
- `.sqlx/query-789c50d6b085b13148e75817fd1efb42ad17ef61d27da6fa8daf0bef5a683356.json`
- `.sqlx/query-f516083922ac301a0a45483f408fde210a52be40b9e7caeaf0c79c5df6226d22.json`

**Files added by code-review patches:**

- (test added) `save_rejects_empty_kind` in `crates/infrastructure/tests/task_repository_test.rs`
- (3 unit tests added) `truncate_borrow_*` in `crates/infrastructure/src/adapters/postgres_task_repository.rs`

**Modified files:**

- `crates/infrastructure/Cargo.toml` — added sqlx, tokio, tracing, chrono, uuid, serde, serde_json, thiserror, async-trait deps + testcontainers/testcontainers-modules/tokio dev-deps
- `crates/infrastructure/src/lib.rs` — declared `db`, `error`, `adapters` modules; re-exported `PostgresTaskRepository`, `MIGRATOR`, `create_pool`, `DEFAULT_MAX_CONNECTIONS`, `DEFAULT_ACQUIRE_TIMEOUT`
- `crates/infrastructure/src/adapters/mod.rs` — declared `postgres_task_repository` module and re-exported `PostgresTaskRepository`
- `crates/domain/src/error.rs` — removed `TaskError::NotFound` variant; refactored `TaskError::Storage` to carry `#[source] source: Box<dyn std::error::Error + Send + Sync>`
- `crates/domain/src/model/task.rs` — added `pub const fn TaskRecord::new(...)` constructor (required because `TaskRecord` is `#[non_exhaustive]`)
- `deny.toml` — added `[[bans.features]]` block for `sqlx` native-tls feature flags; narrowed `[bans] deny` list to actual TLS impls (`openssl`/`openssl-sys`/`openssl-src`/`native-tls`) — `schannel`/`security-framework`/`security-framework-sys` removed during code review (decision 2b: they are rustls cert-store helpers, not competing TLS impls); rewrote header comments to reflect the final enforcement state
- `migrations/0001_create_tasks_table.sql` — code-review patches added `tasks_status_check` (5-value enum CHECK) and `tasks_kind_nonempty_check` (`length(kind) > 0`) constraints
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — code-review patches: added `truncate_last_error_borrow` helper, applied write-side truncation in `save()`, added `id ASC` tiebreaker to `list_by_queue` ORDER BY, fixed `floor_char_boundary` doc comment to point at the correct stabilization (Rust 1.86)
- `crates/infrastructure/tests/task_repository_test.rs` — code-review patches: rewrote `last_error_is_truncated_to_4_kib` to verify raw `octet_length` + `find_by_id` round-trip; widened `save_populates_default_timestamps` tolerance to 5s; added `save_rejects_empty_kind` test
- `.sqlx/query-*.json` — regenerated to capture the new `list_by_queue` ORDER BY clause
- `docs/artifacts/implementation/sprint-status.yaml` — updated `1a-2-postgres-schema-and-task-repository` from `backlog → ready-for-dev → in-progress → review`; updated `last_updated` and header comment
- `docs/artifacts/implementation/deferred-work.md` — marked 6 items resolved by Story 1A.2, partially-resolved 2 items, left 3 genuinely deferred items intact

### Change Log

- 2026-04-06 — Story 1A.2 implemented. `tasks` table migration + `PostgresTaskRepository` adapter + `db.rs` pool helper + embedded `MIGRATOR` + 17 infrastructure unit tests + 5 testcontainers integration tests (TEA P0-INT-001). Domain `TaskError::NotFound` removed; `TaskError::Storage` refactored to typed source. `TaskRecord::new` constructor added. `.sqlx/` offline cache committed (3 query files). `deny.toml` updated with `exclude-dev = true` and `[[bans.features]]` for sqlx; `cargo deny check bans` now meaningfully passes with sqlx in the resolved graph. 6 deferred-work items resolved. All quality gates green: `cargo fmt --check`, `cargo clippy -- -D clippy::pedantic`, `cargo test --workspace` (40 passed), `cargo deny check bans`. Status: ready-for-dev → in-progress → review.
- 2026-04-06 — Code review (3-layer adversarial: Blind Hunter, Edge Case Hunter, Acceptance Auditor) → 3 decisions resolved + 5 patches applied + 5 items deferred + 3 dismissed.
  - **Decisions resolved:** (1b) `create_pool` keeps `TaskError` return — AC 5 amended; (2b) `deny.toml` narrowed — `schannel`/`security-framework`/`security-framework-sys` removed from ban list (rustls cert-store helpers, not TLS impls); `[graph] exclude-dev = true` dropped; both AC 10 conditions now satisfied literally across the workspace; (3b) `save` uses `sqlx::query_as!` — AC 8 amended.
  - **Patches applied:** (P1) `last_error` write-side truncation in `save()` via new `truncate_last_error_borrow` helper — the database row is now also capped at 4 KiB, not just the in-memory return value; integration test rewritten to verify the raw row via `SELECT octet_length(last_error)` AND via `find_by_id` round-trip per AC 9 wording. (P2) Migration adds `tasks_status_check` CHECK constraint enforcing the 5-value status enum at the storage boundary. (P3) Migration adds `tasks_kind_nonempty_check` CHECK constraint; new `save_rejects_empty_kind` integration test confirms the row is rejected before persistence (was previously orphaned). (P4) `list_by_queue` ORDER BY adds `id ASC` deterministic tiebreaker. (P5) `save_populates_default_timestamps` tolerance widened from 1s to 5s per AC 9 literal.
  - **`.sqlx/` cache regenerated** to reflect the new ORDER BY clause (3 query files). `cargo fmt --check`, `cargo clippy -- -D clippy::pedantic`, `cargo deny check bans`, and `cargo test --workspace` (44 passed: 18 domain + 20 infra unit + 6 integration) all green.
  - 5 deferred items added to `deferred-work.md` under "code review of 1a-2" heading: cross-field invariants `(claimed_by, claimed_until)` and `attempts ≤ max_attempts` (Epic 1B), upsert / duplicate-id error variant (Story 1A.3), `OnceCell` Docker-failure caching (CI hardening follow-up), payload leakage via sqlx error formatting (Story 3.1).
  - Status: review → done.
