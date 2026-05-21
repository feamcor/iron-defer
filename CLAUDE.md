# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

iron-defer is a durable background task queue for Rust, backed by Postgres. It guarantees at-least-once execution with automatic retries, jittered backoff, and sweeper-based recovery. It runs as an embedded library (`IronDefer::builder()`) or as a standalone binary with REST API and CLI.

## Build & Development Commands

```bash
# Build
cargo build --workspace

# Lint (pedantic clippy — CI uses -D warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --check

# Supply chain / license / advisory checks
cargo deny check

# SQLx offline query cache check
cargo sqlx prepare --check --workspace

# Run all tests (requires Docker for testcontainers)
cargo test --workspace

# Run a single test
cargo test -p iron-defer --test integration_test -- test_name

# Run a single crate's tests
cargo test -p iron-defer-domain

# Run benchmarks (requires Docker)
cargo bench -p iron-defer --bench throughput
```

Cargo aliases (defined in `.cargo/config.toml`):
- `cargo check-all` → `clippy --workspace --all-targets -- -D clippy::pedantic` (stricter than CI's `-D warnings`)
- `cargo test-all` → `test --workspace`

The full CI quality gate is `fmt --check` + `clippy -D warnings` + `cargo deny check` + `cargo test` + `sqlx prepare --check`. All five must pass before merging.

### Local Postgres for examples/manual testing

```bash
docker compose -f docker/docker-compose.dev.yml up -d
DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer cargo run --example basic_enqueue
```

Examples live in `crates/api/examples/`: `basic_enqueue`, `axum_integration`, `retry_and_backoff`, `multi_queue`. Prefer running an example for manual verification over scratch binaries (see "No Scratch Files Left Behind" below).

### Running the standalone binary

```bash
DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
  cargo run -p iron-defer -- serve --port 8080
```

Other subcommands: `iron-defer submit`, `iron-defer tasks`, `iron-defer config validate`.

### SQLx offline mode

CI sets `SQLX_OFFLINE=true`. After changing any SQL query, regenerate the offline cache:
```bash
cargo sqlx prepare --workspace
```

## Docker Container Cleanup (MANDATORY)

After **every** `cargo test` invocation (whether it passes, fails, or is interrupted), you MUST clean up Docker containers spawned by testcontainers:

```bash
docker ps -aq --filter "label=org.testcontainers=true" | xargs -r docker rm -f 2>/dev/null; docker ps -aq --filter "ancestor=postgres:11-alpine" | xargs -r docker rm -f 2>/dev/null
```

This applies to ALL test commands: single-package tests, workspace-wide tests, integration tests, chaos tests, and any ad-hoc `cargo test` variant. No exceptions.

Do NOT rely on testcontainers' async Drop or Ryuk reaper to clean up — static `OnceCell` container handles and process kills prevent reliable automatic cleanup.

## No Scratch Files Left Behind (MANDATORY)

If you create temporary files or compile scratch binaries (e.g., for quick verification tests), you MUST delete them before finishing the task. Never leave compiled binaries, temp scripts, or throwaway files in the project tree or working directory.

## Architecture

Hexagonal (ports & adapters) with four workspace crates. Dependency flow is strictly unidirectional:

```
domain → application → infrastructure → api
```

- **`crates/domain`** — Pure domain types (`TaskRecord`, `TaskStatus`, `TaskError`, `QueueName`, etc.). No I/O, no framework deps. `#![forbid(unsafe_code)]`.
- **`crates/application`** — Use-case orchestration. Defines port traits (`TaskRepository`, `TaskExecutor`) and services (`SchedulerService`, `WorkerService`, `SweeperService`). Uses `mockall` for unit-testing services without a database.
- **`crates/infrastructure`** — Adapters: `PostgresTaskRepository` (implements `TaskRepository`), observability setup (OTel metrics, tracing, Prometheus), connection pool management. Feature-gated `bin-init` controls global tracing subscriber access.
- **`crates/api`** — Public library façade (`IronDefer`, `IronDeferBuilder`), axum HTTP server, CLI (clap), REST handlers. This is the only crate where logic in `lib.rs` is permitted. Re-exports the public API surface; only `PgPool`, `&'static Migrator`, and `Transaction<'_, Postgres>` may cross the API boundary from sqlx.

### Key architectural constraints

- The caller provides the `PgPool` — iron-defer never creates its own Tokio runtime or connection pool.
- `TaskRegistry` is constructed only in `crates/api/src/lib.rs` (outside of unit tests).
- rustls only — OpenSSL/native-tls are banned via `deny.toml`.
- All crates use `#![forbid(unsafe_code)]`.
- No logic in `lib.rs` except in the `api` crate (which hosts the builder and façade).

### Configuration chain (ADR-0003)

Precedence: defaults < `config.toml` < `config.{profile}.toml` < env vars (`IRON_DEFER__` prefix) < CLI flags.

- Env vars use `__` (double underscore) as the nesting separator, e.g. `IRON_DEFER__DATABASE__URL`, `IRON_DEFER__WORKER__CONCURRENCY`.
- Set `IRON_DEFER_PROFILE=<name>` to layer in `config.{name}.toml` on top of `config.toml`.
- `DATABASE_URL` is also accepted as an alias for `IRON_DEFER__DATABASE__URL`.
- `database.unlogged_tables = true` switches the tasks table to PostgreSQL `UNLOGGED` for high-throughput non-durable workloads. Data is lost on crash recovery and the option is mutually exclusive with `database.audit_log`.

## Test Infrastructure

- Integration tests use `testcontainers` to spin up Postgres. A shared `OnceCell<TestDb>` in `crates/api/tests/common/mod.rs` lazily starts one container per test binary.
- Tests skip gracefully if Docker is unavailable. Set `IRON_DEFER_REQUIRE_DB=1` to fail instead of skip (use this in CI or when you specifically want DB-backed tests to run).
- Use `common::unique_queue()` to scope tests to isolated queue names.
- Use `common::fresh_pool_on_shared_container()` when tests start worker engines (avoids cross-runtime pool hangs).
- Chaos tests (`chaos_*.rs`) simulate worker crashes, DB outages, max retries. They use `#[serial_test]`.

## Reference Documentation

When changing a subsystem, consult the relevant ADR or guideline before making non-trivial changes:

- **ADRs** in `docs/adr/` — 0001 hexagonal architecture, 0002 error handling, 0003 configuration, 0004 async runtime, 0005 sqlx, 0006 serde.
- **Guidelines** in `docs/guidelines/` — `rust-idioms.md`, `quality-gates.md`, `security.md`, `structured-logging.md`, `postgres-reconnection.md`, `compliance-evidence.md`.

## Rust Edition & MSRV

- Edition: 2024
- MSRV: 1.94
- Formatter: `rustfmt.toml` sets `edition = "2024"`, `max_width = 100`

## Commit conventions

Imperative mood, present tense (`Add`, `Fix`, `Remove`). One logical change per PR. New behavior must include tests. See `CONTRIBUTING.md` for the full CI gate list.
