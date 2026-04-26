# Story 1A.1: Workspace Scaffolding & Domain Model

Status: done

## Story

As a Rust developer,
I want a properly structured Cargo workspace with core domain types and the Task trait,
so that I can define task types and build upon a solid architectural foundation.

## Acceptance Criteria

1. **Workspace compiles with 4 crates.** `cargo check --workspace` succeeds from a fresh checkout. Workspace contains exactly: `crates/domain`, `crates/application`, `crates/infrastructure`, `crates/api`. Workspace `Cargo.toml` declares `resolver = "2"`, `edition = "2024"`, and `rust-version = "1.94"` in `[workspace.package]`.

2. **Domain crate exposes the Task trait and core types.** Public API includes:
   - `Task` trait: `const KIND: &'static str;` and `async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError>;` (native `async fn` — no `async-trait` macro on user-facing trait, per architecture line 685–690).
   - Newtypes: `TaskId(Uuid)`, `WorkerId(Uuid)`, `QueueName(String)` (validated — non-empty, no whitespace).
   - `TaskStatus` enum with variants `Pending`, `Running`, `Completed`, `Failed`, `Cancelled` (serde with `rename_all = "snake_case"` to match SQL string values in Architecture D1.1).
   - `TaskRecord` struct with fields exactly matching schema D1.1: `id`, `queue`, `kind`, `payload`, `status`, `priority`, `attempts`, `max_attempts`, `last_error`, `scheduled_at`, `claimed_by`, `claimed_until`, `created_at`, `updated_at`.
   - Typed error enums per ADR-0002: `TaskError`, `ClaimError`, `ValidationError` — all `#[derive(Debug, Error)]` via `thiserror`. No `Box<dyn Error>`. No `anyhow`.

3. **Application crate exposes ports and config (depends only on domain).**
   - `TaskRepository` port trait with async methods: `save(&self, task: &TaskRecord) -> Result<TaskRecord, TaskError>`, `find_by_id(&self, id: TaskId) -> Result<Option<TaskRecord>, TaskError>`, `list_by_queue(&self, queue: &QueueName) -> Result<Vec<TaskRecord>, TaskError>`. Trait must be object-safe (use `async_trait::async_trait` here — port traits need dyn dispatch).
   - `TaskExecutor` port trait (signature TBD by 1B; for this story define it as a stub trait with at minimum an `execute` method placeholder so the layer compiles).
   - Config structs in `config.rs`: `AppConfig`, `WorkerConfig`, `DatabaseConfig`, `ServerConfig`, `ObservabilityConfig`. Empty/default fields are acceptable for this story — they will be populated by later stories.
   - `crates/application/Cargo.toml` lists ONLY `iron-defer-domain` as a workspace-internal dependency. No `infrastructure`, no `api`.

4. **Infrastructure and api crates have correct dependency layering.**
   - `infrastructure/Cargo.toml`: depends on `domain` + `application` (no `api`).
   - `api/Cargo.toml`: depends on `domain` + `application` + `infrastructure`.
   - All workspace dependencies use `{ workspace = true }` form.
   - `crates/api/Cargo.toml` declares `[lib] name = "iron_defer"` and `[[bin]] name = "iron-defer"` for the dual-target setup.

5. **Workspace tooling files exist.**
   - `rustfmt.toml` (`edition = "2024"`, `max_width = 100`).
   - `deny.toml` with an OpenSSL ban in `[bans]` (per architecture line 239 — rustls only).
   - `.gitignore` covering `/target`, `.env`, OS/IDE noise.
   - `.env.example` with placeholder `DATABASE_URL=postgres://...`.
   - `.cargo/config.toml` defining at minimum `[alias] check-all = "clippy --workspace --all-targets -- -D clippy::pedantic"` and `test-all = "test --workspace"`.

6. **Quality gate.** `cargo fmt --check`, `cargo check-all` (the alias above), and `cargo test --workspace` all pass. Domain crate has at least placeholder unit tests covering `QueueName` validation and `TaskStatus` round-trip serialization (TEA: domain crate coverage ≥ 80% — this story establishes the baseline; full coverage compiles as more stories add types).

## Tasks / Subtasks

- [x] **Task 1: Initialize workspace root** (AC: 1, 5)
  - [x] `git init` (already done) — confirm clean working tree on a feature branch
  - [x] Create workspace root `Cargo.toml` with `[workspace]`, `members = ["crates/*"]`, `resolver = "2"`, and the full `[workspace.package]` and `[workspace.dependencies]` blocks copied verbatim from `docs/artifacts/planning/architecture.md` lines 160–211. Do NOT add or remove dependencies — use the architecture's pinned versions.
  - [x] Create `rustfmt.toml`, `deny.toml`, `.gitignore`, `.env.example`, `.cargo/config.toml` per AC 5. For `deny.toml`, include `[bans] deny = [{ name = "openssl" }, { name = "openssl-sys" }, { name = "native-tls" }]`.

- [x] **Task 2: Scaffold the four crates** (AC: 1, 4)
  - [x] `cargo new --lib crates/domain` → set `name = "iron-defer-domain"` in its `Cargo.toml`, inherit `version`, `edition`, `rust-version`, `license`, `repository` from `workspace = true`.
  - [x] Same for `crates/application` (`iron-defer-application`) and `crates/infrastructure` (`iron-defer-infrastructure`).
  - [x] `cargo new --lib crates/api` → set `name = "iron-defer"`, declare BOTH `[lib] name = "iron_defer" path = "src/lib.rs"` AND `[[bin]] name = "iron-defer" path = "src/main.rs"`. Create a stub `src/main.rs` that calls into a placeholder function in `lib.rs` (`fn main() { iron_defer::run_placeholder(); }`) so the binary target compiles.
  - [x] In each crate's `Cargo.toml`, add only the workspace deps actually needed for the types in this story. Domain needs: `thiserror`, `uuid`, `chrono`, `serde`, `serde_json`. Application needs: `domain` (path), `async-trait`, `serde`, `thiserror`. Infrastructure needs: `domain`, `application` (path) — leave it minimal; sqlx wiring is Story 1A.2's job. API needs: all three workspace crates.
  - [x] Run `cargo tree -p iron-defer-application` and verify it does NOT show `infrastructure` or `api`. Run for `infrastructure` and verify it does NOT show `api`.

- [x] **Task 3: Implement domain layer** (AC: 2)
  - [x] `crates/domain/src/lib.rs` — re-exports only, no logic (architecture line 562: "no logic in lib.rs" rule).
  - [x] `crates/domain/src/model/mod.rs` — module declarations.
  - [x] `crates/domain/src/model/task.rs` — define `Task` trait, `TaskStatus` enum, `TaskRecord` struct, `TaskContext` (minimal: holds `task_id: TaskId`, `worker_id: WorkerId`, `attempt: u32` for now). `TaskRecord` derives `Debug, Clone, serde::Serialize, serde::Deserialize`.
  - [x] `crates/domain/src/model/worker.rs` — `WorkerId(Uuid)` newtype with `new() -> Self { Self(Uuid::new_v4()) }`.
  - [x] `crates/domain/src/model/queue.rs` — `QueueName(String)` newtype with `TryFrom<&str>` returning `ValidationError` if empty or contains whitespace; also a `pub fn as_str(&self) -> &str`.
  - [x] Also define `TaskId(Uuid)` newtype — placement: a top-level `model/ids.rs` or alongside `task.rs` (developer's call; pick whichever keeps `task.rs` from getting too long).
  - [x] `crates/domain/src/error.rs` — `TaskError`, `ClaimError`, `ValidationError` enums per ADR-0002 lines 35–57. Use the `TaskError` variants from the ADR (`NotFound`, `AlreadyClaimed`, `InvalidPayload`, `ExecutionFailed`) as the starting set; you may extend with additional variants only if a compile-time gap forces it.
  - [x] Add `#[cfg(test)] mod tests` blocks: (a) `QueueName::try_from("")` returns `ValidationError`; (b) `QueueName::try_from(" foo")` returns `ValidationError`; (c) `QueueName::try_from("payments").unwrap().as_str() == "payments"`; (d) `serde_json::to_string(&TaskStatus::Pending) == "\"pending\""` and round-trip back.

- [x] **Task 4: Implement application layer** (AC: 3)
  - [x] `crates/application/src/lib.rs` — re-exports only.
  - [x] `crates/application/src/ports/mod.rs`, `task_repository.rs`, `task_executor.rs` — port traits as described in AC 3. Use `#[async_trait::async_trait]` on the port traits because they need to be object-safe (`Arc<dyn TaskRepository>`).
  - [x] `crates/application/src/config.rs` — the five config structs. All fields can be plain types; derive `Debug, Clone, serde::Deserialize`. Provide `Default` impls so the rest of the workspace can construct them in tests. **Do NOT** wire up figment loading here — that is `crates/api/src/config.rs`'s job in a later story.
  - [x] No services in this story — `services/` module can be created empty (`mod.rs` with placeholder declarations) or omitted entirely. Prefer omission to avoid dead code warnings.

- [x] **Task 5: Stub infrastructure and api crates** (AC: 4)
  - [x] `crates/infrastructure/src/lib.rs` — re-exports only; create empty `adapters/mod.rs` and `observability/mod.rs` placeholder modules so the directory layout from architecture lines 591–600 exists (real impls land in 1A.2 and Epic 3).
  - [x] `crates/api/src/lib.rs` — placeholder `pub fn run_placeholder() { tracing::info!("iron-defer not yet wired"); }` is acceptable. The real `IronDefer` builder is built in Story 1A.3. Note that `api/src/lib.rs` is the SOLE crate where logic in `lib.rs` is permitted (architecture lines 562–565).
  - [x] `crates/api/src/main.rs` — minimal `fn main() { iron_defer::run_placeholder(); }`.

- [x] **Task 6: Verify quality gates** (AC: 6)
  - [x] `cargo fmt --check`
  - [x] `cargo check --workspace`
  - [x] `cargo clippy --workspace --all-targets -- -D clippy::pedantic` (or `cargo check-all` if the alias is set up)
  - [x] `cargo test --workspace`
  - [x] `cargo deny check bans` — confirms OpenSSL ban configuration parses. (Full `cargo deny check` may fail if licenses/advisories aren't configured; that's fine for this story — only `bans` must pass.)
  - [x] Commit the result.

### Review Findings

_Adversarial code review (2026-04-06): 3 layers — Acceptance Auditor, Blind Hunter, Edge Case Hunter._

**Decision needed (resolve before patches):**

_(none — all 3 decisions deferred to Story 1A.2 boundary; see Deferred section below)_

**Patch (apply now):**

- [x] [Review][Patch] **Silent binary — no tracing subscriber installed** — `run_placeholder()` calls `tracing::info!` but `main()` never installs a subscriber, so the binary's only observable output is silently dropped. [crates/api/src/main.rs:6]
- [x] [Review][Patch] **`QueueName` validation accepts NUL bytes, control chars, and unbounded length** — Postgres TEXT will reject NUL with `invalid byte sequence` far from the input boundary; control / zero-width chars create indistinguishable queue names; no length cap. [crates/domain/src/model/queue.rs:485-495]
- [x] [Review][Patch] **`TaskContext::attempt: u32` mismatches `TaskRecord::attempts: i32`** — future conversion site will need a checked cast. Standardize on `i32` (matches DB) to remove the boundary entirely. [crates/domain/src/model/task.rs:665-679]
- [x] [Review][Patch] **`deny.toml` TLS bans incomplete** — bans `openssl`, `openssl-sys`, `native-tls` but misses `openssl-src` (vendored OpenSSL build), `schannel` (Windows native TLS), and `security-framework` (macOS native TLS). Architecture line 239 says "rustls only". [deny.toml:912-916]
- [x] [Review][Patch] **`crates/application/Cargo.toml` declares `thiserror` but doesn't use it** — no file in the application crate imports `thiserror`; `TaskError` lives in the domain crate. Unused dep. [crates/application/Cargo.toml:18]
- [x] [Review][Patch] **Misleading MSRV-1.94 comment in `Task` trait doc** — comment implies native `async fn` in trait requires 1.94, but it stabilized in 1.75. Document the actual reason 1.94 is the MSRV (or remove the parenthetical). [crates/domain/src/model/task.rs:685]
- [x] [Review][Patch] **`#![forbid(unsafe_code)]` missing from `crates/api/src/main.rs`** — `forbid` in `lib.rs` does not propagate to the `bin` target's crate root. Unsafe could be introduced in `main.rs` without tripping the lint. [crates/api/src/main.rs]
- [x] [Review][Patch] **`TaskContext` and `TaskRecord` not `#[non_exhaustive]`** — both are public domain types in a library crate; adding fields later (Epic 1B will extend `TaskContext` per its own doc comment) will be a source-breaking change for downstream `Task` implementations. [crates/domain/src/model/task.rs:642,665]
- [x] [Review][Patch] **`Default for TaskId` / `Default for WorkerId` silently allocates randomness** — `..Default::default()` patterns will produce a fresh random ID per call, which is surprising and footgun-prone for an identifier type. Remove the `Default` impls; force callers to write `TaskId::new()` explicitly. [crates/domain/src/model/task.rs:614, crates/domain/src/model/worker.rs:31]

**Deferred (real but out of 1A.1 scope):**

- [x] [Review][Defer] **`TaskExecutor::execute(&TaskRecord)` lacks `TaskContext` and bridge to `Task::execute`** [crates/application/src/ports/task_executor.rs:14] — deferred, story explicitly defers executor surface to Epic 1B
- [x] [Review][Defer] **`DatabaseConfig::url` empty default + `max_connections` ceiling unenforced** [crates/application/src/config.rs:205-212] — deferred, story spec defers figment loading and FR41 ceiling enforcement to later stories
- [x] [Review][Defer] **`TaskRecord::last_error: Option<String>` no length cap** [crates/domain/src/model/task.rs:653] — deferred, no consumer in 1A.1; adapter-level concern for Story 1A.2
- [x] [Review][Defer] **`crates/infrastructure/Cargo.toml` missing `tracing`/`sqlx`/etc.** [crates/infrastructure/Cargo.toml:13-15] — deferred, exactly Story 1A.2 territory; already noted in Dev Agent Record
- [x] [Review][Defer] **`.gitignore` lacks OS/IDE noise (`.DS_Store`, `.idea/`, etc.)** [.gitignore] — deferred, pre-existing file from `cargo new`; not introduced by this story
- [x] [Review][Defer] **Workspace declares ~13 unused deps → `cargo deny` rustls-only enforcement currently unverifiable** [Cargo.toml:64-95] — deferred, intentional pre-staging per story spec ("copy verbatim from architecture lines 160–211"); the deny rule passes trivially today because none of `sqlx`/`reqwest`/etc. are in the resolved graph yet. Story 1A.2 must re-run `cargo deny check bans` once `sqlx` lands and confirm rustls-only is actually enforced.
- [x] [Review][Defer] **`TaskError::NotFound` is unreachable via current `find_by_id` signature** [crates/application/src/ports/task_repository.rs:21] — deferred, `find_by_id` returns `Ok(None)` for absence so `NotFound` is dead at this API surface. Reconcile in Story 1A.2: either remove the variant or change `find_by_id` to `Result<TaskRecord, TaskError>`.
- [x] [Review][Defer] **`deny.toml` should add `[[bans.features]]` for reqwest/sqlx native-tls features** [deny.toml] — deferred until 1A.2 actually pulls those crates into the graph; feature bans only fire on resolved deps.
- [x] [Review][Defer] **`TaskRecord` has no constructor / validation for `attempts`, `max_attempts`, `priority`, `kind`** [crates/domain/src/model/task.rs:642-658] — deferred to Story 1A.2: validate at the `TryFrom<TaskRow>` boundary in `PostgresTaskRepository`. Hexagonal "validate at the edge" principle. Story 1A.2 may also revisit whether `TaskRecord` fields should become `pub(crate)` with a checked builder for the `enqueue` API.
- [x] [Review][Defer] **`Task` trait missing `Serialize + DeserializeOwned` supertrait bounds (Architecture D7.2)** [crates/domain/src/model/task.rs:689] — deferred to Epic 1B: the bounds matter when the registry serializes payloads for type-erased dispatch. Tightening now would require updating the inline `DummyTask` test for no immediate benefit.
- [x] [Review][Defer] **`TaskError::{InvalidPayload, ExecutionFailed, Storage}` use stringly-typed `reason: String` instead of typed sources** [crates/domain/src/error.rs:19-27] — deferred to Story 1A.2: introduce typed source variants per ADR-0002 once `PostgresTaskRepository` exists and we know which `sqlx::Error` shapes the adapter actually needs to distinguish (unique violation, connection timeout, schema mismatch, etc.). Speculative typing now would likely miss real cases.

## Dev Notes

### Architecture Source of Truth

- **Workspace structure & Cargo.toml content:** `docs/artifacts/planning/architecture.md` §"Project Initialization" (lines 129–242). The dependency versions there are normative — do not bump them.
- **Crate module layouts:** architecture.md §"Structure Patterns" (lines 568–615). Domain has `lib.rs` + `model/{task,worker,queue}.rs` + `error.rs`. The full directory tree is also at lines 840–921.
- **Layer dependency rules:** architecture.md lines 925–934. Enforced by Cargo crate boundaries — `cargo tree` reveals violations.
- **Tasks table schema (for `TaskRecord` field list):** architecture.md §D1.1 (lines 269–305). Field names and types are Story 1A.2's responsibility for SQL; this story only models them in Rust.
- **Error handling:** `docs/adr/0002-error-handling.md` lines 35–101 — `TaskError` shape, layering rules, `From` impl pattern.
- **Hexagonal layering:** `docs/adr/0001-hexagonal-architecture.md`.
- **MSRV 1.94 + native async fn in traits:** architecture.md lines 685–690. `Task` trait uses native `async fn`; only the registry/port traits need `async_trait` for object-safety.
- **TLS = rustls only, OpenSSL banned:** architecture.md lines 235–239.

### Critical Conventions (do NOT deviate)

- **No logic in `lib.rs`** for `domain`, `application`, `infrastructure` — only `pub use` re-exports (architecture line 562). `crates/api/src/lib.rs` is the sole exception.
- **Crate package names use kebab-case `iron-defer-{layer}`**, but `[lib] name` uses snake_case (`iron_defer_domain`, etc.) — this matches Rust conventions and the architecture's `[lib] name = "iron_defer"` for the api crate.
- **No `unwrap()` in production code**; `expect("invariant: ...")` only for documented invariants; both freely allowed in `#[cfg(test)]` (architecture lines 768–770).
- **No `anyhow` in any library crate** (architecture line 773). Use `thiserror`-derived enums.
- **Workspace deps inheritance:** every crate uses `{ workspace = true }` — never re-declare a version.
- **`TaskRow` (DB row) types are `pub(crate)`** in infrastructure (architecture line 948). Not relevant this story but worth noting before 1A.2.
- **`#[instrument(skip(self), fields(...), err)]` on every public async method in application + infrastructure** (architecture lines 692–702). The port traits in this story have async methods — add `#[instrument]` to their implementations later, but no impls exist yet, so nothing to instrument in this story. Mention this in your notes for 1A.2.

### Out of Scope for This Story

- No SQLx, no migrations, no Postgres connection — Story 1A.2.
- No `IronDefer` builder, no `enqueue()` API — Story 1A.3.
- No worker pool, no claiming SQL, no `TaskHandlerAdapter` — Epic 1B.
- No HTTP server, no CLI, no figment config loading — later epics.
- No real `TaskExecutor` impl — just the trait stub so the application crate compiles.

### Tooling Notes

- `cargo new --lib` creates a `Cargo.toml` with `[package]` defaults — replace `version`, `edition`, `rust-version`, `license`, `repository` with `{ workspace = true }` inheritance after generation.
- `cargo new` also creates `src/lib.rs` with a default test — keep it or replace with the real test from Task 3.
- For the `[[bin]]` declaration in `crates/api/Cargo.toml`, you must explicitly set `path = "src/main.rs"` because declaring `[lib] path = "src/lib.rs"` disables Cargo's autodetection.

### Project Structure Notes

- The crate directory layout matches architecture.md lines 840–921 exactly.
- Workspace member globbing (`members = ["crates/*"]`) is acceptable and matches the architecture's intent; an explicit list of four members is also fine if you prefer the verbosity.
- `migrations/`, `docker/`, `k8s/`, `.github/workflows/`, `.sqlx/` directories from the architecture tree are NOT created in this story — they belong to later stories. Do not pre-create empty versions.

### References

- [Source: docs/artifacts/planning/architecture.md#Project-Initialization (lines 129–242)]
- [Source: docs/artifacts/planning/architecture.md#Implementation-Patterns (lines 536–779)]
- [Source: docs/artifacts/planning/architecture.md#Project-Structure (lines 781–956)]
- [Source: docs/artifacts/planning/architecture.md#D1.1-Tasks-Table-Schema (lines 269–305)]
- [Source: docs/artifacts/planning/epics.md#Story-1A.1 (lines 252–289)]
- [Source: docs/adr/0001-hexagonal-architecture.md]
- [Source: docs/adr/0002-error-handling.md (lines 35–101)]

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context)

### Debug Log References

- `cargo check --workspace` — clean (4 crates)
- `cargo fmt --all -- --check` — clean (after auto-format pass)
- `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean (3 doc-markdown fixes applied during dev: backticks around `PostgreSQL`, `OTel`, `OTLP`)
- `cargo test --workspace` — 11 passed, 0 failed
- `cargo build --workspace --all-targets` — clean (lib + bin)
- `cargo deny check bans` — `bans ok`
- `cargo tree -p iron-defer-application` — confirmed no `iron-defer-infrastructure` or `iron-defer` (api) deps
- `cargo tree -p iron-defer-infrastructure` — confirmed no `iron-defer` (api) dep

### Completion Notes List

- All 6 ACs satisfied. Workspace compiles with the four hexagonal crates (`domain`, `application`, `infrastructure`, `api`), Rust 2024 edition, MSRV 1.94, `resolver = "2"`.
- Domain crate ships `Task` trait using **native `async fn` in trait** (no `async-trait` macro on the user-facing trait, per architecture lines 685–690). The `TaskRepository` and `TaskExecutor` ports in the application crate DO use `#[async_trait]` because they need to be object-safe for `Arc<dyn …>` injection.
- `TaskStatus` enum uses `#[serde(rename_all = "snake_case")]` so the Rust variants serialize to the lowercase strings (`"pending"`, `"running"`, …) that match the SQL `tasks.status` column from architecture §D1.1. Story 1A.2 will rely on this when wiring up the SQLx adapter.
- `QueueName` is a validated newtype: rejects empty strings and any whitespace via `ValidationError::EmptyQueueName` / `ValidationError::QueueNameWhitespace`. Both `TryFrom<&str>` and `TryFrom<String>` are implemented; serde uses the `String` form via `#[serde(try_from = "String", into = "String")]` so deserialization also enforces the invariants.
- `TaskRecord` field types: `priority: i16` and `attempts: i32` / `max_attempts: i32` chosen to map directly to PostgreSQL `SMALLINT` and `INTEGER` (architecture §D1.1) so 1A.2 can use `sqlx::FromRow` without conversions. `claimed_by` is `Option<WorkerId>`, `claimed_until` and `last_error` are `Option`s — matching the nullable columns in the schema.
- Application config structs (`AppConfig`, `DatabaseConfig`, `WorkerConfig`, `ServerConfig`, `ObservabilityConfig`) all derive `Default` so tests can construct them freely. They are pure data only — figment loading lives in `crates/api/src/config.rs` (deferred to a later story).
- Stub `TaskExecutor` trait declared with a placeholder `execute(&self, &TaskRecord)` method so the application crate has no compile gaps. The real executor surface is Epic 1B's responsibility.
- `crates/api/src/lib.rs` ships only `pub fn run_placeholder()` — the sole crate where logic in `lib.rs` is permitted (architecture lines 562–565). Real `IronDefer` builder is Story 1A.3.
- Empty `crates/infrastructure/src/{adapters,observability}/mod.rs` directories created so the directory layout from architecture lines 873–879 is locked in; concrete impls land in 1A.2 and Epic 3.
- `deny.toml` enforces only the OpenSSL ban for now — `cargo deny check bans` passes. Full license/advisory configuration is deferred to a later story (Epic 5 production-readiness).
- `.cargo/config.toml` provides the `check-all` and `test-all` aliases per architecture line 798–801.
- TaskExecutor and TaskRepository port methods are not yet `#[instrument]`'d because no implementations exist in this story. Story 1A.2 must add `#[instrument(skip(self), fields(task_id = %id, queue = %queue), err)]` on the `PostgresTaskRepository` impl methods (architecture lines 692–702).
- Note for Story 1A.2: the `tracing` crate is currently only listed in the `api` crate's `Cargo.toml`. When the infrastructure adapter starts emitting spans, add `tracing = { workspace = true }` to `crates/infrastructure/Cargo.toml`.

### File List

**New files:**

- `Cargo.toml` (workspace root)
- `Cargo.lock`
- `rustfmt.toml`
- `deny.toml`
- `.env.example`
- `.cargo/config.toml`
- `crates/domain/Cargo.toml`
- `crates/domain/src/lib.rs`
- `crates/domain/src/error.rs`
- `crates/domain/src/model/mod.rs`
- `crates/domain/src/model/task.rs`
- `crates/domain/src/model/worker.rs`
- `crates/domain/src/model/queue.rs`
- `crates/application/Cargo.toml`
- `crates/application/src/lib.rs`
- `crates/application/src/config.rs`
- `crates/application/src/ports/mod.rs`
- `crates/application/src/ports/task_repository.rs`
- `crates/application/src/ports/task_executor.rs`
- `crates/infrastructure/Cargo.toml`
- `crates/infrastructure/src/lib.rs`
- `crates/infrastructure/src/adapters/mod.rs`
- `crates/infrastructure/src/observability/mod.rs`
- `crates/api/Cargo.toml`
- `crates/api/src/lib.rs`
- `crates/api/src/main.rs`

**Modified files:**

- `.gitignore` — added `.env` exclusion (kept `.env.example` tracked)

### Change Log

- 2026-04-06 — Story 1A.1 implemented. Workspace scaffolding + domain model + application ports + infrastructure & api stubs. All quality gates green (`cargo check`, `cargo fmt --check`, `cargo clippy -- -D clippy::pedantic`, `cargo test`, `cargo deny check bans`). 11 unit tests passing. Status: ready-for-dev → in-progress → review.
- 2026-04-06 — Code review (3-layer adversarial): 9 patches applied (silent binary fix via `tracing-subscriber` init, `QueueName` validation hardened against NUL/control/zero-width/bidi chars + 128-byte length cap, `TaskContext::attempt` aligned to `i32`, `deny.toml` extended with `openssl-src`/`schannel`/`security-framework`, removed unused `thiserror` dep from application crate, fixed misleading MSRV-1.94 doc comment, added `#![forbid(unsafe_code)]` to `main.rs`, added `#[non_exhaustive]` to `TaskContext` and `TaskRecord`, removed `Default` impls from `TaskId`/`WorkerId`). 8 new QueueName edge-case tests added (19 total tests passing). 8 findings deferred to Story 1A.2 / Epic 1B (see Review Findings + `deferred-work.md`). Status: review → done.
