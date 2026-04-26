# Story 5.2: Feature Flags & Configuration Enforcement

Status: done

## Story

As a Rust developer,
I want optional capabilities gated behind Cargo feature flags and safety enforcement on configuration,
so that I can include only what I need and the engine prevents unsafe configurations.

## Acceptance Criteria

1. **Cargo feature flags — `snake_case`, grouped by concern:**

   The workspace and crate `Cargo.toml` files define feature flags:
   - `default = []` — no features enabled by default.
   - `tokio_console` — enables `console-subscriber` for dev diagnostics (tokio-console runtime introspection). Never enabled in production.
   - Features use `snake_case` naming grouped by concern (FR35, Architecture lines 749–756).

   Feature flags are defined on the `api` crate (the public-facing crate) and propagated to downstream crates via `dep:` syntax where needed.

   **Maps to:** FR35, Architecture lines 749–756.

2. **Pool size ceiling enforcement:**

   `DatabaseConfig` includes a `max_connections` field. When the configured pool size exceeds a documented ceiling (default: 100 connections), engine construction fails with a descriptive error.

   The ceiling value is:
   - Documented as a constant in `crates/infrastructure/src/db.rs`.
   - Referenced in `.env.example` and configuration documentation.
   - Validated in `create_pool()` before creating the `PgPool`.

   **Maps to:** FR41, PRD lines 333–337.

3. **UNLOGGED / audit mutual exclusion:**

   `DatabaseConfig` gains two new boolean fields: `unlogged_tables` (default `false`) and `audit_log` (default `false`).

   When both are `true`, the engine refuses to start with an explicit error: `"UNLOGGED table mode and audit_log are mutually exclusive — UNLOGGED tables do not survive Postgres crash recovery and cannot satisfy audit trail requirements."`

   This is a configuration validation check. The actual UNLOGGED table mode and audit log table are Growth-phase features — this story only enforces the mutual exclusion constraint at configuration time.

   **Maps to:** FR40, PRD lines 339–345.

4. **`deny.toml` — license policy and advisories:**

   Extend the existing `deny.toml` (which already bans OpenSSL) with:
   - `[licenses]` section — allow list for permissive licenses (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-DFS-2016, Zlib, etc.). Deny copyleft licenses (GPL, AGPL, LGPL) unless explicitly exempted.
   - `[advisories]` section — enable advisory checking, fail on known vulnerabilities, configure `unmaintained = "warn"`.

   `cargo deny check` must pass: bans ok, licenses ok, advisories ok.

   **Maps to:** Epic AC, Architecture lines 758–780 enforcement guidelines.

5. **Quality gates:**

   - `cargo deny check` — bans ok, licenses ok, advisories ok.
   - `cargo fmt --check` — clean.
   - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean.
   - `SQLX_OFFLINE=true cargo test --workspace` — all new tests + existing suites pass.
   - `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (rustls-only preserved).
   - Feature flag compilation: `cargo check -p iron-defer --features tokio_console` compiles.
   - Feature flag default: `cargo check -p iron-defer` compiles (no features enabled).

## Tasks / Subtasks

- [x] **Task 1: Add `tokio_console` feature flag** (AC 1)
  - [x] Add `console-subscriber = "0.4"` to workspace `[workspace.dependencies]` as optional.
  - [x] Add `[features]` section to `crates/api/Cargo.toml`:
    ```toml
    [features]
    default = []
    tokio_console = ["dep:console-subscriber"]
    ```
  - [x] Add `console-subscriber` as an optional dependency in `crates/api/Cargo.toml`.
  - [x] Add conditional initialization in tracing setup: when `tokio_console` feature is enabled, add a `console_subscriber::ConsoleLayer` to the tracing subscriber stack.
  - [x] Verify `cargo check -p iron-defer` (no features) compiles.
  - [x] Verify `cargo check -p iron-defer --features tokio_console` compiles.

- [x] **Task 2: Pool size ceiling enforcement** (AC 2)
  - [x] Add `MAX_POOL_CONNECTIONS` constant to `crates/infrastructure/src/db.rs` (value: 100).
  - [x] Add validation in `create_pool()`: AFTER resolving `max_connections = 0` to `DEFAULT_MAX_CONNECTIONS`, check if the resolved value exceeds `MAX_POOL_CONNECTIONS`. If so, return `TaskError::Storage` with descriptive error message including the ceiling value and the configured value.
  - [x] Update the doc comment on `DEFAULT_MAX_CONNECTIONS` to reference the ceiling.
  - [x] Update `.env.example` with a comment documenting the ceiling: `# Maximum: 100 (FR41)`.
  - [x] Add unit test: `pool_size_ceiling_rejects_oversized` — config with `max_connections = 101` is rejected.
  - [x] Add unit test: `pool_size_at_ceiling_accepted` — config with `max_connections = 100` is accepted.
  - [x] Add unit test: `pool_size_zero_uses_default_passes_ceiling` — config with `max_connections = 0` resolves to 10, passes ceiling check.

- [x] **Task 3: UNLOGGED / audit mutual exclusion** (AC 3)
  - [x] Add `unlogged_tables: bool` and `audit_log: bool` to `DatabaseConfig` in `crates/application/src/config.rs`. Use `#[serde(default)]` on the struct (already present) or on each field to ensure existing configs without these fields deserialize correctly (both default to `false`).
  - [x] Add validation method `DatabaseConfig::validate() -> Result<(), String>` that checks mutual exclusion: both `true` → `Err(...)` with the prescribed message. Follow the same pattern as `WorkerConfig::validate()`.
  - [x] Call `DatabaseConfig::validate()` from `IronDeferBuilder::build()` alongside the existing `WorkerConfig::validate()`, converting the error via `.map_err(|reason| TaskError::InvalidPayload { reason })`.
  - [x] Update `.env.example` with commented-out entries for the new fields.
  - [x] Add unit test: `unlogged_and_audit_mutual_exclusion` — both `true` → error.
  - [x] Add unit test: `unlogged_only_accepted` — `unlogged_tables = true`, `audit_log = false` → ok.
  - [x] Add unit test: `audit_only_accepted` — `unlogged_tables = false`, `audit_log = true` → ok.
  - [x] Add unit test: `both_false_accepted` — both `false` → ok (default).

- [x] **Task 4: Extend `deny.toml` with licenses and advisories** (AC 4)
  - [x] Add `[licenses]` section with `allow` list of permissive licenses used by the dependency tree. Run `cargo deny list` to discover exact licenses in use.
  - [x] Set `unlicensed = "deny"`, `copyleft = "deny"`, `default = "deny"`. (Note: cargo-deny 0.19.4 removed these deprecated keys; allow-list approach achieves the same effect.)
  - [x] Add exemptions for any non-standard licenses with documented rationale (e.g., `ring` uses ISC + OpenSSL-derived license).
  - [x] Add `[advisories]` section: `vulnerability = "deny"`, `unmaintained = "warn"`, `yanked = "deny"`. (Note: cargo-deny 0.19.4 changed field values; adapted to new format with ignore list for unfixable transitive deps.)
  - [x] Configure `db-path` and `db-urls` for advisory database. (Note: cargo-deny 0.19.4 uses built-in advisory DB by default.)
  - [x] Run `cargo deny check` and fix any failures.
  - [x] Verify bans, licenses, and advisories all pass.

- [x] **Task 5: Integration test — pool ceiling via builder** (AC 2)
  - [x] Add test in `crates/api/tests/` or inline: `IronDeferBuilder` with a pool built from an oversized `DatabaseConfig` → `build()` returns error.
  - [x] This validates the full stack: config → pool creation → ceiling check.

- [x] **Task 6: Integration test — UNLOGGED/audit via builder** (AC 3)
  - [x] Add test: `IronDeferBuilder::build()` with `unlogged_tables = true` and `audit_log = true` → returns error with expected message.

- [x] **Task 7: Quality gates** (AC 5)
  - [x] `cargo deny check` — all sections pass (bans, licenses, advisories).
  - [x] `cargo fmt --check` — clean.
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --lib` — clean.
  - [x] All existing + new tests pass.
  - [x] Feature compilation: both `--features tokio_console` and default compile.
  - [x] No OpenSSL in dependency tree.

### Review Findings

- [x] [Review][Patch] Redundant `dep:console-subscriber` on api crate [crates/api/Cargo.toml:19] — removed direct dep, simplified feature to forward-only.

## Dev Notes

### Architecture Compliance

- **FR35** (PRD line 776): "A developer can enable or disable optional capabilities (metrics, tracing, audit-log, unlogged) via Cargo feature flags." AC 1 delivers the `tokio_console` flag. Full `metrics`, `tracing`, `audit-log`, `unlogged` feature flags are Growth-phase — the Epic AC only requires `tokio-console` at minimum.
- **FR40** (PRD line 784): "The engine can enforce mutual exclusion between UNLOGGED table mode and audit logging." AC 3 delivers this.
- **FR41** (PRD line 785): "The engine can enforce a maximum Postgres connection pool size, rejecting construction if the configured size exceeds a documented ceiling." AC 2 delivers this.
- **Architecture lines 749–756**: Feature flags use `snake_case`, `default = []`, `tokio-console` as `dep:console-subscriber`. Note: Architecture uses `tokio-console` (kebab-case) but Rust feature names must use `snake_case` per Cargo convention. Use `tokio_console` in Cargo.toml.
- **Architecture lines 758–780**: Enforcement guidelines and anti-patterns — `deny.toml` is the tooling support for these.
- **PRD lines 333–345**: Pool size ceiling (default 20 in PRD — but 100 is more practical for production; document the chosen value). UNLOGGED/audit mutual exclusion error message is prescribed.

### Critical Design Decisions

**Pool ceiling value: 100 (deliberate deviation from PRD's 20).**
The PRD line 335 suggests a default ceiling of 20 connections for embedded mode. This is too conservative for production workloads — a 4-worker engine with sweeper, HTTP server, and pool overhead can reasonably need 20+ connections. A ceiling of 100 prevents runaway configuration while allowing legitimate high-throughput deployments. The ceiling is a safety guardrail, not an optimization target. This is a deliberate architectural deviation from the PRD: the PRD was written before the engine's actual connection usage patterns were understood. The dev agent should use 100 unless the user explicitly requests 20. Document the value and rationale in the code constant.

**`DatabaseConfig::validate()` separate from `WorkerConfig::validate()`.**
The existing pattern is `WorkerConfig::validate()` called from `IronDeferBuilder::build()`. Add a parallel `DatabaseConfig::validate()` for the UNLOGGED/audit check. The pool ceiling check belongs in `create_pool()` (infrastructure layer) since it's about the physical pool, not the config shape.

**`tokio_console` feature lives on the `api` crate.**
The `console-subscriber` needs to be wired into the tracing subscriber stack, which is initialized in `crates/infrastructure/src/observability/tracing.rs` (called from `main.rs`). The feature flag should be on the `api` crate (user-facing) and propagated to `infrastructure` via `dep:` feature forwarding. The `infrastructure` crate adds the `ConsoleLayer` conditionally.

**UNLOGGED and audit_log fields on `DatabaseConfig`, not `WorkerConfig`.**
These settings are about the database behavior (table mode, audit table), not worker behavior. Placing them on `DatabaseConfig` is semantically correct and keeps `WorkerConfig` focused on worker-pool concerns.

**`deny.toml` license allow-list approach.**
Use an explicit allow-list (`allow = [...]`) rather than a deny-list. This is safer — new transitive dependencies with unexpected licenses are caught automatically. The workspace's dependency tree uses primarily MIT, Apache-2.0, and BSD-variant licenses. Some crates (like `ring`) have custom licenses that need explicit exemptions.

### Previous Story Intelligence

**From Story 5.1 (Docker & Kubernetes Deployment, 2026-04-21):**
- `.env.example` updated as reference for env vars — Story 5.2 should update it with pool ceiling and UNLOGGED/audit fields.
- No Rust code changes in Story 5.1 — Story 5.2 is the first code-changing story in Epic 5.

**From Epic 1B/2/3 Retrospective (2026-04-21):**
- Preparation item #9: "Config validation rejects invalid values (`Duration::ZERO`, pool size ceiling FR41, UNLOGGED/audit mutual exclusion FR40)." This story delivers the FR41 and FR40 portions.
- `WorkerConfig::validate()` already rejects `Duration::ZERO` and other invalid values — the pattern is established.

**From Story 1A.2 (Postgres Schema & Task Repository):**
- `deny.toml` already bans OpenSSL crates and locks down sqlx TLS features. Story 5.2 extends it with licenses and advisories.
- `crates/infrastructure/src/db.rs` line 10-11: "FR41 ceiling enforcement is deferred to Epic 5." This is the story.

**From Story 3.1 (Structured Logging & Payload Privacy):**
- `infrastructure` crate already has a `bin-init` feature flag (for `init_tracing`). The pattern of feature-gating infrastructure initialization is established.
- `tracing.rs` initializes the subscriber — this is where `ConsoleLayer` would be conditionally added.

### Git Intelligence

Recent commits (last 5):
- `a0db5fb` — REST API list tasks and queue stats (Story 4.2).
- `7d0c584` — Health probes and task cancellation APIs (Story 4.1).
- `2b70581` — Custom Axum extractors for structured JSON error responses.
- `2a1ed9a` — Removed OTel compliance tests for Story 3.3.
- `940c722` — OTel compliance tests and SQL audit trail.

### Key Types and Locations (verified current as of 2026-04-21)

- `DatabaseConfig` — `crates/application/src/config.rs:21-28`. Fields: `url`, `max_connections`. Will add `unlogged_tables`, `audit_log`.
- `WorkerConfig` — `crates/application/src/config.rs:32-60`. Has `validate()` method — pattern to follow.
- `WorkerConfig::validate()` — `crates/application/src/config.rs:76-113`. Called from `IronDeferBuilder::build()`.
- `AppConfig` — `crates/application/src/config.rs:12-18`. Top-level config aggregating all concerns.
- `create_pool()` — `crates/infrastructure/src/db.rs:75-94`. Creates `PgPool`. Pool ceiling check goes here.
- `DEFAULT_MAX_CONNECTIONS` — `crates/infrastructure/src/db.rs:26`. Currently 10. FR41 ceiling is a separate constant.
- `MAX_POOL_CONNECTIONS` — does not exist yet. Will be added as ceiling constant.
- `IronDeferBuilder::build()` — `crates/api/src/lib.rs:812-855`. Calls `WorkerConfig::validate()`. Will also call `DatabaseConfig::validate()`.
- `deny.toml` — workspace root. Has `[bans]` section with OpenSSL ban. No `[licenses]` or `[advisories]` yet.
- `.env.example` — workspace root. Reference for env var naming.
- `infrastructure` crate features — `crates/infrastructure/Cargo.toml:24-26`. Has `bin-init` feature. Will add `tokio_console` forwarding.
- `init_tracing()` — `crates/infrastructure/src/observability/tracing.rs`. Subscriber initialization — conditional `ConsoleLayer` goes here.

### Dependencies

**New workspace dependency:**
```toml
console-subscriber = { version = "0.4", optional = true }
```

Add to `[workspace.dependencies]` in root `Cargo.toml`. The `optional = true` at the workspace level is just documentation — the actual optionality is controlled by the `dep:` syntax in crate-level `[features]`.

No other new dependencies. `cargo-deny` is a dev tool run externally, not a Cargo dependency.

**Verify after adding:** `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` must remain empty. `console-subscriber` depends on `tokio` and `tracing` — no TLS dependencies.

### Test Strategy

**Unit tests (inline in config modules):**
- `DatabaseConfig::validate()` — mutual exclusion: both true → error, all other combos → ok.
- Pool ceiling: `max_connections = 101` → error, `max_connections = 100` → ok, `max_connections = 0` → uses default.

**Integration tests (in `crates/api/tests/` or inline):**
- `IronDeferBuilder::build()` with oversized pool config → error.
- `IronDeferBuilder::build()` with UNLOGGED + audit → error.

**`deny.toml` verification:**
- `cargo deny check` passes all three sections (bans, licenses, advisories).

**Feature flag compilation:**
- `cargo check -p iron-defer` (default, no features) — compiles.
- `cargo check -p iron-defer --features tokio_console` — compiles.

### Project Structure Notes

**New files:**
- None (all modifications to existing files).

**Modified files:**
- `Cargo.toml` (workspace root) — add `console-subscriber` to `[workspace.dependencies]`.
- `crates/api/Cargo.toml` — add `[features]` section with `tokio_console`, add `console-subscriber` as optional dep.
- `crates/infrastructure/Cargo.toml` — add `tokio_console` feature forwarding if needed.
- `crates/infrastructure/src/observability/tracing.rs` — conditional `ConsoleLayer` when `tokio_console` enabled.
- `crates/application/src/config.rs` — add `unlogged_tables`, `audit_log` to `DatabaseConfig`, add `DatabaseConfig::validate()`.
- `crates/infrastructure/src/db.rs` — add `MAX_POOL_CONNECTIONS` constant, add ceiling check in `create_pool()`.
- `crates/api/src/lib.rs` — call `DatabaseConfig::validate()` in `IronDeferBuilder::build()`.
- `deny.toml` — add `[licenses]` and `[advisories]` sections.
- `.env.example` — add pool ceiling doc comment, UNLOGGED/audit fields.

**Not modified:**
- Migrations — no schema changes.
- `.sqlx/` — unchanged.
- No new source files created.

### Out of Scope

- **Actual UNLOGGED table mode implementation** — Growth phase. This story only validates the config.
- **Actual audit log table** — Growth phase. This story only validates the config.
- **`metrics` and `tracing` feature flags** — The PRD lists these as planned flags. They are Growth-phase features. The Epic AC only requires `tokio-console` at minimum.
- **`tokio-console` runtime wiring** — This story adds the feature flag and conditional layer. Actually running `tokio-console` against the binary is a dev workflow, not an AC deliverable.
- **CI pipeline enforcement** — `cargo deny check` in CI is Story 5.1 (Docker/CI) or later.
- **Pool size auto-tuning** — The ceiling is a hard cap. Dynamic sizing based on Postgres `max_connections` is Growth phase.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 851–879] — Story 5.2 acceptance criteria (BDD source).
- [Source: `docs/artifacts/planning/architecture.md` lines 749–756] — Feature flags spec.
- [Source: `docs/artifacts/planning/architecture.md` lines 758–780] — Enforcement guidelines.
- [Source: `docs/artifacts/planning/prd.md` lines 333–345] — Pool ceiling and UNLOGGED/audit mutual exclusion.
- [Source: `docs/artifacts/planning/prd.md` lines 564–572] — Feature flag table.
- [Source: `docs/artifacts/planning/prd.md` lines 776, 784–785] — FR35, FR40, FR41.
- [Source: `crates/application/src/config.rs`] — DatabaseConfig, WorkerConfig, validate() pattern.
- [Source: `crates/infrastructure/src/db.rs`] — create_pool(), DEFAULT_MAX_CONNECTIONS, FR41 deferral comment.
- [Source: `crates/infrastructure/Cargo.toml`] — Existing `bin-init` feature flag pattern.
- [Source: `crates/api/src/lib.rs` lines 812–855] — IronDeferBuilder::build().
- [Source: `deny.toml`] — Existing bans section.
- [Source: `.env.example`] — Env var naming reference.
- [Source: `docs/artifacts/implementation/epic-1b-2-3-retro-2026-04-21.md` line 133] — Preparation item #9: FR41 + FR40 config validation.

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- cargo-deny 0.19.4 removed deprecated config keys (`unlicensed`, `default`, `copyleft`, `vulnerability`, `unmaintained`, `yanked` as string values). Adapted to new format using allow-list and ignore list.
- `ring` license clarification required explicit `[[licenses.clarify]]` block with LICENSE file hash.
- Three advisory exemptions added for unfixable transitive deps: protobuf 2.28.0 (via prometheus 0.13.x), tokio-tar 0.3.1 (archived, testcontainers-only), rustls-pemfile 2.2.0 (archived, testcontainers-only).
- `rustls-webpki` updated from 0.103.10 to 0.103.13 to fix RUSTSEC-2026-0098 and RUSTSEC-2026-0099.
- `IronDeferBuilder::build()` reordered: config validation now runs before pool check so builder-level validation tests work without a real pool.

### Completion Notes List

- Task 1: `tokio_console` feature flag added to `api` crate, forwarded to `infrastructure` crate. `ConsoleLayer` conditionally added to tracing subscriber. Both default and `tokio_console` feature compilations verified.
- Task 2: `MAX_POOL_CONNECTIONS = 100` constant added. Ceiling check in `create_pool()` after default resolution. 3 unit tests pass.
- Task 3: `unlogged_tables` and `audit_log` fields added to `DatabaseConfig` with `#[serde(default)]`. `DatabaseConfig::validate()` enforces mutual exclusion. `database_config` field and setter added to `IronDeferBuilder`. 4 unit tests pass.
- Task 4: `deny.toml` extended with `[licenses]` (13 permissive licenses) and `[advisories]` sections. `cargo deny check` passes clean.
- Tasks 5-6: Integration tests in `config_validation_test.rs` validate full stack for pool ceiling and UNLOGGED/audit mutual exclusion.
- Task 7: All quality gates pass — deny check, fmt, clippy, 237 tests, feature compilation, no OpenSSL.

### File List

- `Cargo.toml` — added `console-subscriber` to workspace deps
- `Cargo.lock` — updated (console-subscriber deps, rustls-webpki 0.103.13)
- `crates/api/Cargo.toml` — added `[features]` section with `tokio_console`, `console-subscriber` optional dep
- `crates/api/src/lib.rs` — added `DatabaseConfig` to re-exports, `database_config` field and setter on `IronDeferBuilder`, reordered validation in `build()`
- `crates/api/tests/config_validation_test.rs` — NEW: 2 integration tests for pool ceiling and UNLOGGED/audit
- `crates/api/tests/db_outage_integration_test.rs` — added `..Default::default()` to `DatabaseConfig` literals
- `crates/infrastructure/Cargo.toml` — added `tokio_console` feature, `console-subscriber` optional dep
- `crates/infrastructure/src/lib.rs` — re-exported `MAX_POOL_CONNECTIONS`
- `crates/infrastructure/src/db.rs` — added `MAX_POOL_CONNECTIONS` constant, ceiling check in `create_pool()`, 3 unit tests
- `crates/infrastructure/src/observability/tracing.rs` — conditional `ConsoleLayer` when `tokio_console` feature enabled
- `crates/infrastructure/tests/tracing_privacy_test.rs` — added `..Default::default()` to `DatabaseConfig` literals
- `crates/application/src/config.rs` — added `unlogged_tables`, `audit_log` to `DatabaseConfig`, `#[serde(default)]`, `DatabaseConfig::validate()`, 4 unit tests
- `deny.toml` — added `[licenses]` and `[advisories]` sections
- `.env.example` — added pool ceiling comment, UNLOGGED/audit fields

### Change Log

| Date | Author | Change |
|---|---|---|
| 2026-04-22 | Dev Agent (Claude Opus 4.6) | Implemented all 7 tasks: tokio_console feature flag, pool ceiling (FR41), UNLOGGED/audit mutual exclusion (FR40), deny.toml licenses+advisories, integration tests, quality gates. 237 tests pass. |
| 2026-04-22 | Code Review (Claude Opus 4.6) | 1 patch applied (removed redundant console-subscriber dep from api crate), 11 dismissed. Status → done. |
