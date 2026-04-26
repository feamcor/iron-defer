---
stepsCompleted: [1, 2, 3, 4, 5, 6, 7, 8, 'growth-addendum']
lastStep: 'growth-addendum'
status: 'complete'
completedAt: '2026-04-04'
reconciledAt: '2026-04-24'
growthAddendumAt: '2026-04-24'
inputDocuments:
  - 'docs/artifacts/planning/prd.md'
  - 'docs/artifacts/planning/prd-validation-report.md'
  - 'docs/artifacts/planning/research/domain-distributed-task-queue-durable-execution-research-2026-04-02.md'
  - 'docs/adr/0001-hexagonal-architecture.md'
  - 'docs/adr/0002-error-handling.md'
  - 'docs/adr/0003-configuration-management.md'
  - 'docs/adr/0004-async-runtime-tokio-ecosystem.md'
  - 'docs/adr/0005-database-layer-sqlx.md'
  - 'docs/adr/0006-serialization-serde.md'
  - 'docs/guidelines/rust-idioms.md'
  - 'docs/guidelines/quality-gates.md'
workflowType: 'architecture'
project_name: 'iron-defer'
user_name: 'Fabio'
date: '2026-04-03'
---

# Architecture Decision Document

_This document builds collaboratively through step-by-step discovery. Sections are appended as we work through each architectural decision together._

## Project Context Analysis

### Requirements Overview

**Functional Requirements (17 capabilities across 5 categories):**

1. **Task Lifecycle Management:** Submit tasks (REST POST /tasks), retrieve status
   (GET /tasks/{id}), cancel pending tasks (DELETE /tasks/{id}), inspect queue (GET /tasks).
   State machine: Pending → Running → Completed | Failed.

2. **Distributed Execution Engine:** Atomic task claiming via SKIP LOCKED (no duplicate
   processing under normal conditions). Worker pool with configurable concurrency.
   Task abstraction (HTTP webhooks, shell commands, embedded Rust functions via Task trait).

3. **Resilience & Recovery:** Sweeper/Reaper recovering zombie tasks (Running + expired
   lease). Configurable retry with exponential backoff. SIGTERM graceful shutdown (complete
   in-flight or release leases cleanly).

4. **Observability:** OTel metrics (queue depth, execution latency, retry rate, failure rate).
   Structured logging with task_id correlation via tracing spans. Payload not logged by default
   (log_payload: false config).

5. **Dual Deployment Modes:** Embedded library (integrates into caller's Tokio runtime via
   lib.rs). Standalone binary (Docker/Kubernetes, main.rs). Both modes share the same
   Postgres-backed engine. CLI for operator task submission and queue inspection.

**Non-Functional Requirements:**

| NFR | Target | Measurement |
|-----|--------|-------------|
| Throughput | ≥ 10,000 jobs/sec | Benchmark suite, single Postgres instance |
| Task recovery | < lease duration (configurable, e.g. 5 min) | Chaos integration tests |
| Zero task loss | 100% recovery in chaos tests | testcontainers-rs chaos suite |
| Time-to-first-task | < 30 minutes | Developer onboarding docs + demo |
| OTel coverage (MVP) | Metrics + Logs | Test suite against OTel Collector |
| At-least-once guarantee | Zero duplicate completions under normal conditions | Integration test assertions |
| Graceful shutdown | In-flight tasks complete or leases released on SIGTERM | Integration tests |
| PostgreSQL minimum | 14+ | CI matrix |
| Rust MSRV | 1.80+ stable | CI matrix |
| No new infrastructure | Zero dependencies beyond PostgreSQL | Design constraint |

**Scale & Complexity:**

- Primary domain: Systems infrastructure — backend library + service binary
- Complexity level: High / Enterprise
- Estimated major architectural components: 18–22
- Regulatory frameworks: 7 (PCI DSS, GDPR, HIPAA, DORA, NIS2, SOC 2 CC7.2, ISO 27001:2022)

### Technical Constraints & Dependencies

**Pre-committed (via ADRs 0001–0006) — not open for re-decision:**
- Cargo workspace with 4-crate hexagonal architecture: domain / application / infrastructure / api
- Tokio as sole async runtime; no async-std, no smol
- PostgreSQL 14+ as sole runtime dependency; no message brokers
- SQLx for database access (compile-time verified queries, embedded migrations)
- axum for HTTP server; reqwest for HTTP client
- figment + dotenvy + clap for layered configuration
- serde for all serialization (per-category attribute conventions)
- thiserror per library layer; color-eyre at binary boundary
- No OpenSSL — rustls only
- clippy::pedantic + cargo deny + cargo audit + cargo tarpaulin (80% domain coverage) as CI gates

**Open architectural decisions (to be made in subsequent steps):**
- Postgres schema design for the tasks table and state machine transitions
- SKIP LOCKED claiming and lease management protocol
- Sweeper/Reaper scheduling and isolation model
- Worker pool concurrency model (bounded, back-pressure)
- OTel integration points and metric naming conventions
- Embedded vs. standalone entry-point wiring
- Graceful shutdown coordination across components
- API versioning and error response contract
- Security architecture consolidation (TLS, no-auth MVP boundary)

### Cross-Cutting Concerns Identified

1. **Observability** — tracing spans + OTel metrics/logs must propagate across every layer
   boundary. Every async method in application and infrastructure layers requires
   `#[instrument(err, fields(task_id = ...))]`.

2. **Error propagation** — typed errors per layer (ADR-0002) must compose cleanly across
   the claim → execute → retry → recover cycle without losing context across async boundaries.

3. **Distributed concurrency** — SKIP LOCKED claiming, lease expiry detection, and
   Sweeper recovery must be architecturally consistent. Race conditions between concurrent
   workers are the primary correctness risk.

4. **Graceful shutdown** — SIGTERM must propagate through the worker pool, sweeper, and
   API server in a coordinated way. No orphaned Running tasks on planned shutdown.

5. **Testability** — hexagonal port/adapter model (ADR-0001) enables unit testing of
   application logic without a database. Integration tests require testcontainers for
   infrastructure layer. Chaos tests (kill workers mid-execution) are acceptance criteria.

6. **Security** — TLS for Postgres (rustls), payload privacy (no payload logging by default),
   no secrets in log output. REST API has no auth in MVP — must be explicitly documented
   as a gap for compliance auditors.

7. **Dual-mode wiring** — embedded library must accept a caller-provided PgPool and not
   spawn its own Tokio runtime. Standalone binary owns the runtime and pool. Both must
   share the same application-layer engine without code duplication.

## Project Initialization

### Primary Technology Domain

Rust systems infrastructure library with Cargo workspace architecture.
No conventional web framework starter applies — initialization is manual workspace scaffolding.

### Starter Approach: Manual Cargo Workspace Initialization

**Rationale:** The hexagonal 4-crate workspace structure (ADR-0001) and full technology
stack (ADRs 0002–0006) are already committed. No existing Rust workspace generator
matches this exact structure. Manual initialization gives full control and avoids
generator assumptions that conflict with established ADRs.

**Initialization Commands:**

```bash
# Workspace root
mkdir iron-defer && cd iron-defer
git init

# Workspace Cargo.toml (manual — see structure below)
# Crates
cargo new --lib crates/domain
cargo new --lib crates/application
cargo new --lib crates/infrastructure
cargo new --lib crates/api        # will also add main.rs for dual-target
```

**Workspace Root `Cargo.toml`:**

```toml
[workspace]
members = [
    "crates/domain",
    "crates/application",
    "crates/infrastructure",
    "crates/api",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.94"
license = "MIT OR Apache-2.0"
repository = "https://github.com/feamcor/iron-defer"

[workspace.dependencies]
# Internal crates
iron-defer-domain = { path = "crates/domain" }
iron-defer-application = { path = "crates/application" }
iron-defer-infrastructure = { path = "crates/infrastructure" }

# Builder derive
bon = "3"

# Domain
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Infrastructure
sqlx = { version = "0.8", features = [
    "runtime-tokio-rustls", "postgres", "uuid", "chrono", "json", "migrate"
] }
tokio = { version = "1" }
axum = { version = "0.8" }
reqwest = { version = "0.12", features = ["rustls-tls"], default-features = false }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
opentelemetry = "0.27"
opentelemetry_sdk = { version = "0.27", features = ["metrics", "rt-tokio"] }
opentelemetry-otlp = { version = "0.27", features = ["metrics", "http-proto", "reqwest-client"] }
opentelemetry-prometheus = "0.27"
prometheus = "0.13"

# Config
figment = { version = "0.10", features = ["toml", "env"] }
dotenvy = "0.15"
clap = { version = "4", features = ["derive", "env"] }

# Errors
color-eyre = "0.6"

# Async traits (until async fn in traits fully stable across all use cases)
async-trait = "0.1"

# Utilities
tokio-util = { version = "0.7", features = ["rt"] }
humantime-serde = "1"
rand = "0.9"

# OpenAPI
utoipa = { version = "5", features = ["chrono", "uuid"] }

# tokio-console (dev diagnostics)
console-subscriber = "0.4"

# Benchmarks
criterion = { version = "0.5", features = ["html_reports"] }

# Dev
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["postgres"] }
mockall = "0.13"
tracing-test = "0.2"
```

**Dual-Target `crates/api/Cargo.toml`:**

```toml
[package]
name = "iron-defer"
# inherits workspace.package

[lib]
name = "iron_defer"
path = "src/lib.rs"

[[bin]]
name = "iron-defer"
path = "src/main.rs"
```

**Architectural Decisions Established by Initialization:**

- Rust **2024 edition** across all crates
- MSRV **1.94** declared in `workspace.package.rust-version`
- `resolver = "2"` for correct feature unification in workspaces
- Workspace-level dependency versions — individual crates use `{ workspace = true }`
- **rustls** as the sole TLS implementation across all dependencies (no OpenSSL):
  - `sqlx`: `runtime-tokio-rustls` feature
  - `reqwest`: `rustls-tls` feature, `default-features = false`
  - Any future TLS-capable dependency must follow the same pattern
- OpenSSL banned via `deny.toml` `[bans]` section (per quality gates)

**Note:** Project initialization is the first implementation story. The workspace
`Cargo.toml` and crate scaffolding should be committed before any feature work begins.

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions (Block Implementation):**
- D1.1 Tasks table schema — drives all persistence, claiming, and retry logic
- D2.1 Atomic claiming strategy — correctness of at-least-once guarantee
- D6.1 Graceful shutdown signaling — required for lease release on SIGTERM
- D7.1/D7.2 Public library API surface — external contract, hard to change post-release

**Important Decisions (Shape Architecture):**
- D1.2 Retry/backoff formula — directly affects failure recovery UX
- D2.2 Worker pool concurrency model — affects throughput and back-pressure
- D3.1 Sweeper architecture — affects recovery latency and observability
- D5.1 OTel metric naming — public contract for operators

**Deferred Decisions (Post-MVP):**
- LISTEN/NOTIFY for near-instant task pickup (Growth)
- `task_history` separate audit table (Growth)
- REST API authentication: bearer token / mTLS / OIDC (Growth)
- W3C trace-context propagation across enqueue/dequeue boundary (Growth)
- Partitioning via pg_partman for high-volume deployments (Growth)

### Data Architecture

**D1.1 — Tasks Table Schema**

```sql
CREATE TABLE tasks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    queue         TEXT NOT NULL DEFAULT 'default',
    kind          TEXT NOT NULL,
    payload       JSONB NOT NULL DEFAULT '{}',
    status        TEXT NOT NULL DEFAULT 'pending',
    priority      SMALLINT NOT NULL DEFAULT 0,
    attempts      INTEGER NOT NULL DEFAULT 0,
    max_attempts  INTEGER NOT NULL DEFAULT 3,
    last_error    TEXT,
    scheduled_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_by    UUID,
    claimed_until TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Claiming index: pending tasks eligible for pickup, ordered by priority + scheduled_at
CREATE INDEX idx_tasks_claiming
    ON tasks (queue, status, priority DESC, scheduled_at ASC)
    WHERE status = 'pending';

-- Sweeper index: running tasks with expired leases
CREATE INDEX idx_tasks_zombie
    ON tasks (status, claimed_until)
    WHERE status = 'running';
```

- `queue`: multi-queue support in MVP (default queue = `'default'`)
- `kind`: task type discriminator — handler looked up from a runtime registry keyed by `kind` string
- `priority`: included in MVP; higher value = picked first within a queue
- `status` values: `pending`, `running`, `completed`, `failed`, `cancelled`
- Completed/failed tasks retained in `tasks` table (MVP) — serves as the audit trail.
  `task_history` table deferred to Growth phase.

**D1.2 — Retry / Exponential Backoff Formula**

**Per-task retry delay** (on task failure):
```
next_scheduled_at = now() + min(base_delay * 2^(attempts - 1), max_delay)
```

**Poll-loop jittered backoff** (on consecutive claim failures):
```
next_delay = base_delay + random(0..base_delay)
```
where `base_delay` doubles on each consecutive error, capped at `max_delay`.
Backoff resets on successful claim. This prevents thundering herd when multiple
workers experience simultaneous failures.

Defaults (all configurable per `WorkerConfig`):
- `base_delay`: 5 seconds
- `max_delay`: 30 minutes
- `max_attempts`: 3

Tasks exceeding `max_attempts` transition to `failed` status (not retried further).

**D1.3 — Task Retention**

MVP: Completed and failed tasks remain in the `tasks` table indefinitely.
This table serves as the queryable audit trail satisfying PCI DSS Req. 10 and SOC 2 CC7.2.
Growth: Separate append-only `task_history` table with tamper-evident log semantics.

### Concurrency & Worker Pool

**D2.1 — Atomic Claiming Strategy**

Single-query atomic claim using `UPDATE ... WHERE id = (subquery with SKIP LOCKED) RETURNING *`:

```sql
UPDATE tasks
SET
    status        = 'running',
    claimed_by    = $1,              -- worker UUID
    claimed_until = now() + $2,      -- lease duration interval
    attempts      = attempts + 1,
    updated_at    = now()
WHERE id = (
    SELECT id FROM tasks
    WHERE queue = $3
      AND status = 'pending'
      AND scheduled_at <= now()
    ORDER BY priority DESC, scheduled_at ASC
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
RETURNING *;
```

No rows returned = no available task (worker sleeps until next poll interval).
This is the River pattern — proven at ~10,000 jobs/sec on commodity Postgres.

**D2.2 — Worker Pool Concurrency Model**

`tokio::task::JoinSet` for task lifecycle tracking + `tokio::sync::Semaphore` for bounded concurrency:

- Semaphore with `concurrency` permits controls max simultaneous workers
- Worker acquires permit before claiming; releases permit on completion/failure
- `JoinSet` tracks all in-flight handles for clean drain on shutdown
- Polling via `tokio::time::interval` (configurable, default: **500ms**)

**D2.3 — Polling Strategy**

Interval-based polling for MVP (default 500ms, configurable).
LISTEN/NOTIFY deferred to Growth phase as a latency optimization.

### Sweeper / Reaper

**D3.1 — Sweeper Architecture**

Separate `tokio::spawn`'d task with its own `tokio::time::interval` (configurable, default: **60 seconds**).
Independent of the worker pool — not embedded in the claim loop.

Zombie recovery query:

```sql
-- Recover tasks within retry budget
UPDATE tasks
SET
    status        = 'pending',
    claimed_by    = NULL,
    claimed_until = NULL,
    scheduled_at  = now() + $1,     -- immediate or short backoff
    updated_at    = now()
WHERE status = 'running'
  AND claimed_until < now()
  AND attempts < max_attempts
RETURNING id;

-- Fail tasks that have exhausted retries
UPDATE tasks
SET
    status     = 'failed',
    last_error = 'lease expired: max attempts exhausted',
    updated_at = now()
WHERE status = 'running'
  AND claimed_until < now()
  AND attempts >= max_attempts;
```

Sweeper emits `iron_defer_zombie_recoveries_total` counter on each recovery run.

### Security Architecture

**D4.1 — REST API Authentication**

MVP: No authentication. Intended for internal / private network deployment only.
This is a documented compliance gap — operators must place iron-defer behind a network
boundary (VPC, service mesh, firewall) in any production deployment.
Growth: Bearer token / mTLS / OIDC — decision deferred.

**D4.2 — Request Body Size Limit**

Default: **1 MiB** via axum `DefaultBodyLimit`. Configurable via `ServerConfig`.

**D4.3 — Secrets and Payload Privacy**

- Database URL: never logged (connection string contains credentials)
- Task payload: not logged by default (`log_payload: false` in `WorkerConfig`)
- Worker credentials / API keys in task payloads: caller responsibility via payload encryption
- Spans: `skip(payload)` on all instrumented methods unless `log_payload = true`

### OTel Integration

**D5.1 — Metric Names and Types**

All metrics use the `iron_defer_` prefix (OTLP-compatible, Prometheus-compatible):

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `iron_defer_tasks_pending` | Gauge | `queue` | Current pending task count |
| `iron_defer_tasks_running` | Gauge | `queue` | Current running task count |
| `iron_defer_task_duration_seconds` | Histogram | `queue`, `kind`, `status` | Task execution duration |
| `iron_defer_task_attempts_total` | Counter | `queue`, `kind` | Cumulative attempt count |
| `iron_defer_task_failures_total` | Counter | `queue`, `kind` | Cumulative failure count |
| `iron_defer_zombie_recoveries_total` | Counter | `queue` | Tasks recovered by sweeper |
| `iron_defer_worker_pool_utilization` | Gauge | `queue` | active / max workers ratio |

**D5.2 — OTel SDK Integration**

MVP scope: metrics + structured logs via `opentelemetry-sdk` + `opentelemetry-otlp` (OTLP/gRPC export).
`tracing-subscriber` with JSON formatter for log output.
`tracing-opentelemetry` bridge for trace propagation: Growth phase.

### Graceful Shutdown

**D6.1 — Shutdown Signaling**

`tokio_util::sync::CancellationToken` for tree-structured cancellation:

```
root_token
├── worker_pool_token   (cloned from root)
└── sweeper_token       (cloned from root)
```

SIGTERM handler cancels root token. Workers finish in-flight tasks, release Semaphore
permits, then JoinSet drains. Sweeper completes current cycle and exits.

Drain timeout: **30 seconds** (configurable via `WorkerConfig.shutdown_timeout`).
After timeout: remaining leases released via UPDATE (claimed_by = NULL, status = pending),
process exits.

### Public Library API Surface

**D7.1 — Embedded Library Entry Point**

```rust
let engine = IronDefer::builder()
    .pool(pg_pool)                               // caller-provided PgPool — no runtime spawned
    .concurrency(4)
    .poll_interval(Duration::from_millis(500))
    .sweeper_interval(Duration::from_secs(60))
    .register::<MyTask>()                        // registers KIND string → handler
    .build()
    .await?;

// Enqueue from anywhere in the caller's code
engine.enqueue("default", MyTask { ... }).await?;

// Start workers + sweeper (blocks until shutdown signal)
engine.start().await?;
```

The embedded engine must:
- Accept a caller-provided `PgPool` — never construct its own
- Not spawn a Tokio runtime — caller owns the runtime
- Run migrations on `build()` (opt-out via `.skip_migrations(true)`)

**D7.2 — Task Trait**

```rust
pub trait Task: Send + Sync + Serialize + DeserializeOwned + 'static {
    const KIND: &'static str;
    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError>;
}

// Actual TaskContext (evolved from original spec):
#[non_exhaustive]
pub struct TaskContext {
    pub(crate) task_id: TaskId,
    pub(crate) worker_id: WorkerId,     // added: worker identity for retry bookkeeping
    pub(crate) attempt: AttemptCount,    // changed: i32-backed newtype (matches DB schema)
}
```

**Deviation from original spec:** `TaskContext` dropped the `queue: String` field and added
`worker_id: WorkerId`. The `attempt` type changed from `u32` to `AttemptCount` (an `i32`
newtype matching the Postgres `INTEGER` column). All fields are `pub(crate)` with typed
accessors: `task_id()`, `worker_id()`, `attempt()`.

Note: `async fn in traits` is stable in Rust 1.75+. With MSRV 1.94, `async-trait` proc-macro
is used only for object-safe trait dispatch. Dispatch is through a registry of
`Arc<dyn TaskHandler>` (see C4). Concrete task types implement `Task` using native
`async fn execute` — no `async-trait` needed for user-facing task implementations.

### Decision Impact Analysis

**Implementation Sequence:**
1. Cargo workspace initialization + crate scaffolding
2. Domain model: `Task`, `TaskId`, `WorkerId`, `TaskStatus`, error types
3. Application ports: `TaskRepository`, `TaskExecutor` traits + `TaskRegistry`
4. Postgres schema migration + `PostgresTaskRepository` adapter
5. Claiming engine (atomic SKIP LOCKED query)
6. Worker pool (JoinSet + Semaphore + poll loop)
7. Sweeper task
8. OTel metrics integration
9. REST API (axum) + CLI (clap) adapters
10. Graceful shutdown (CancellationToken wiring)
11. Embedded library façade (`IronDefer` builder)
12. Standalone binary wiring (`main.rs`)
13. Chaos integration tests (testcontainers-rs)

**Cross-Component Dependencies:**
- Worker pool depends on: TaskRepository port, TaskRegistry, CancellationToken, Semaphore, DispatchContext, Metrics
- Sweeper depends on: TaskRepository port (zombie query), CancellationToken, Metrics (in `application/services/sweeper.rs`)
- REST API depends on: TaskRepository port (enqueue, inspect, cancel), utoipa for OpenAPI spec
- OTel metrics: `application/metrics.rs` defines the `Metrics` struct; `infrastructure/observability/metrics.rs` initializes the meter provider
- Graceful shutdown coordinates: worker pool JoinSet, sweeper handle, axum server shutdown

## Implementation Patterns & Consistency Rules

### Critical Conflict Points: 9 identified and resolved

### Naming Patterns

**Database / SQL:**
- Tables and columns: `snake_case`
- Indexes: `idx_{table}_{purpose}` (e.g. `idx_tasks_claiming`, `idx_tasks_zombie`)
- Migration files: `{NNN}_{verb}_{noun}.sql` (e.g. `0001_create_tasks_table.sql`)

**REST API:**
- Plural resource nouns: `/tasks`, `/tasks/{id}`, `/queues/{queue}/tasks`
- Path parameters: `{id}` style (axum `Path<Uuid>`), never `:id`
- Query parameters: `snake_case` (e.g. `?queue=default&status=pending&limit=50`)
- No `/v1/` prefix in MVP — defer API versioning to Growth phase

**Rust identifiers:**
- Port traits (application layer): `Task{Verb}` — `TaskRepository`, `TaskExecutor`, `TaskRegistry`
- Adapter structs (infrastructure layer): `Postgres{PortName}` — `PostgresTaskRepository`
- Error enums: `{Concept}Error` per domain concept — `TaskError`, `ClaimError`, `ScheduleError`
- Config structs: `{Concern}Config` — `WorkerConfig`, `DatabaseConfig`, `ObservabilityConfig`
- OTel instrument constants: `SCREAMING_SNAKE_CASE`; metric string names: `iron_defer_snake_case`

### Structure Patterns

**"No logic in lib.rs" rule scope:** Applies to `domain`, `application`, and `infrastructure`
crates only. `crates/api/src/lib.rs` IS the implementation — it contains the `IronDefer`
builder and embedded library façade. This is the sole exception to the no-logic rule.

**Module layout within each crate:**

```
crates/domain/src/
├── lib.rs              # re-exports only; no logic
├── model/
│   ├── mod.rs
│   ├── task.rs         # TaskRecord, TaskStatus, TaskContext, CancelResult, ListTasksFilter
│   ├── worker.rs       # WorkerId
│   ├── queue.rs        # QueueName newtype
│   ├── kind.rs         # TaskKind newtype
│   ├── attempts.rs     # AttemptCount, MaxAttempts newtypes
│   └── priority.rs     # Priority newtype
└── error.rs            # TaskError, PayloadErrorKind, ExecutionErrorKind, ValidationError

crates/application/src/
├── lib.rs              # re-exports only; no logic
├── config.rs           # AppConfig, WorkerConfig, DatabaseConfig, ObservabilityConfig, ServerConfig
├── metrics.rs          # OTel metric instrument handles (shared across services)
├── ports/
│   ├── mod.rs
│   ├── task_repository.rs  # TaskRepository trait
│   └── task_executor.rs    # TaskExecutor trait
├── registry.rs         # TaskHandler trait + TaskRegistry (kind string → Arc<dyn TaskHandler>)
└── services/
    ├── mod.rs
    ├── scheduler.rs    # SchedulerService: enqueue, cancel, inspect
    ├── worker.rs       # WorkerService (pool) + DispatchContext
    └── sweeper.rs      # SweeperService (extracted from worker.rs)

crates/infrastructure/src/
├── lib.rs              # re-exports only; no logic
├── db.rs               # PgPool creation helper (create_pool, max_connections, connect_timeout)
├── error.rs            # PostgresAdapterError (Query, Configuration, DatabaseScrubbed, Mapping)
├── adapters/
│   ├── mod.rs
│   └── postgres_task_repository.rs
└── observability/
    ├── mod.rs
    ├── metrics.rs      # OTel meter + all iron_defer_* instruments
    └── tracing.rs      # tracing-subscriber + JSON formatter init

crates/api/src/
├── lib.rs              # IronDefer builder + embedded library façade (IS implementation)
├── main.rs             # standalone binary entry point
├── cli/                # clap CLI (modular)
│   ├── mod.rs          # Cli enum + Serve subcommand
│   ├── submit.rs       # task submission commands
│   ├── tasks.rs        # task inspection commands
│   ├── workers.rs      # worker management commands
│   ├── db.rs           # database commands (migrate, etc.)
│   ├── config.rs       # config display commands
│   └── output.rs       # output formatting helpers
├── config.rs           # figment config loading chain (6-step precedence)
├── shutdown.rs         # CancellationToken, OS signals, drain timeout, JoinSet coordination
├── http/
│   ├── mod.rs
│   ├── router.rs       # axum Router + middleware + OpenAPI spec endpoint via utoipa
│   ├── extractors.rs   # custom axum extractors
│   ├── handlers/
│   │   ├── mod.rs
│   │   ├── tasks.rs    # POST/GET/DELETE /tasks, GET /tasks/{id}
│   │   ├── health.rs   # GET /health/live, GET /health/ready
│   │   ├── queues.rs   # GET /queues/{queue}/stats
│   │   └── metrics.rs  # GET /metrics (Prometheus scrape)
│   └── errors.rs       # AppError + IntoResponse + ErrorResponse
```

**Test file placement:**
- Domain + Application unit tests: inline `#[cfg(test)] mod tests { ... }` within source files
- Infrastructure integration tests: `crates/infrastructure/tests/` (3 test files, use testcontainers)
- API integration + chaos tests: `crates/api/tests/` (21 test files, flat layout — no subdirectories)
- Chaos tests are prefixed `chaos_*.rs` with shared `chaos_common.rs` helper; each spins its own container

**Migration files:** `migrations/` at workspace root, numbered sequentially.
Migrations run once inside test DB initialization (`OnceCell::get_or_init`), never per-test.

### Format Patterns

**REST API — direct responses, no envelope wrapper:**

```json
// Success — return the resource directly
// POST /tasks → 201 Created
{ "id": "uuid", "queue": "default", "kind": "PaymentWebhook", "status": "pending",
  "priority": 0, "attempts": 0, "scheduledAt": "2026-04-04T12:00:00Z", "createdAt": "..." }

// Collection — GET /tasks → 200 OK
{ "tasks": [...], "total": 42, "limit": 50, "offset": 0 }

// Error — always this shape
{ "error": { "code": "TASK_NOT_FOUND", "message": "task abc123 not found" } }
```

**Error codes:** `SCREAMING_SNAKE_CASE` — e.g. `TASK_NOT_FOUND`, `TASK_ALREADY_CLAIMED`,
`INVALID_PAYLOAD`, `QUEUE_UNAVAILABLE`, `MAX_RETRIES_EXCEEDED`

**HTTP status codes:**

| Situation | Status |
|---|---|
| Created successfully | 201 |
| Retrieved / listed | 200 |
| Accepted (async enqueue) | 202 |
| Not found | 404 |
| Conflict (already claimed / completed) | 409 |
| Invalid input | 422 |
| Internal error | 500 |
| DB unavailable | 503 |

**Date/time in JSON:** ISO 8601 UTC — `"2026-04-04T12:00:00Z"`. Never Unix timestamps.
**JSON field naming:** `camelCase` (ADR-0006: `rename_all = "camelCase"` on all API structs).
**Task IDs in URLs:** UUID v4, lowercase hyphenated — `/tasks/550e8400-e29b-41d4-a716-446655440000`

### Process Patterns

**TaskRegistry ownership — CRITICAL:**
The registry is constructed once in the `api` crate (binary entry point or `lib.rs` façade),
wrapped in `Arc<TaskRegistry>`, and injected as a dependency into the worker pool and sweeper.
It is **never** constructed in `application` or `infrastructure` crates.
Missing registration = runtime panic with an explicit descriptive message (never silent task drop).

**`shutdown.rs` responsibilities:**
`crates/api/src/shutdown.rs` owns:
- The root `CancellationToken` (created once at startup, cloned into all subsystems)
- OS signal handling (`tokio::signal::ctrl_c` + `SIGTERM` via `tokio::signal::unix`)
- Drain timeout enforcement (configurable, default 30s)
- `JoinSet` drain coordination for worker pool handles

It is not a utility module — it is a first-class orchestration component.

**Public library API boundary:**
`crates/api/src/lib.rs` must not re-export any infrastructure crate types. The sole
exception: `sqlx::PgPool` is accepted as input to the builder (caller-provided).
No other `sqlx`, `axum`, or `reqwest` types may appear in the public API.

**Task trait and async dispatch (MSRV 1.94):**
- Concrete task types implement `Task` using native `async fn execute` — no `async-trait` needed
- The registry stores type-erased handlers as `Arc<dyn TaskHandler>` where `TaskHandler`
  is a separate object-safe trait (or boxed async closure) — boxing happens inside the
  registry, not in user-facing task implementations
- `async-trait` proc-macro is **not required** for user task types at MSRV 1.94

**Tracing instrumentation — every public async method in application and infrastructure:**

```rust
#[instrument(skip(self), fields(task_id = %id, queue = %queue), err)]
pub async fn claim_next(&self, queue: &str) -> Result<Option<Task>, ClaimError> { ... }
// Rules:
// - skip(self) always
// - fields() lists key business identifiers
// - err auto-records errors in the span
// - never include payload in fields unless WorkerConfig.log_payload = true
```

**Error conversion — explicit `From` impls, never discard context:**

```rust
// In infrastructure adapter — translate to domain error at the boundary
impl From<PostgresAdapterError> for TaskError { ... }
// In adapter method: .map_err(PostgresAdapterError::from)?.try_into()
// FORBIDDEN: .map_err(|_| TaskError::Unknown)  — never discard error context
```

**testcontainers — shared DB per test binary (NOT per test):**

```rust
// crates/infrastructure/tests/common/mod.rs
use tokio::sync::OnceCell;

static TEST_DB: OnceCell<(PgPool, ContainerAsync<Postgres>)> = OnceCell::const_new();

pub async fn test_pool() -> &'static PgPool {
    let (pool, _container) = TEST_DB.get_or_init(|| async {
        let container = Postgres::default().start().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let url = format!("postgres://postgres@localhost:{}/postgres", port);
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        (pool, container)
    }).await;
    pool
}
// Rules:
// - container held in static for lifetime of test binary (drop = DB dies)
// - migrations run once inside get_or_init, never per individual test
// - individual tests call test_pool(), never spin up their own container
```

**`fresh_pool_on_shared_container()` — per-test pool isolation:**

```rust
// crates/infrastructure/tests/common/mod.rs
pub async fn fresh_pool_on_shared_container() -> Option<PgPool> { ... }
```

Creates a new `PgPool` on the same shared testcontainer, running migrations independently.
Use when a test needs a clean database state without starting a new container. Preferred
for test binaries with multiple test functions that would otherwise share and pollute
state through the shared `test_pool()`.

The `IRON_DEFER_REQUIRE_DB=1` environment variable makes tests fail-fast when Docker is
unavailable instead of silently skipping (opt-in for CI enforcement).

**Chaos test minimum manifest (`crates/api/tests/chaos_*.rs`):**

Chaos tests are flat files in `crates/api/tests/` (not a subdirectory), prefixed `chaos_`.
Shared setup lives in `chaos_common.rs`. Each test spins its own container.
All four scenarios are required acceptance criteria for the at-least-once guarantee:
1. `chaos_worker_crash_test.rs` — worker killed mid-execution → sweeper recovers task within lease duration
2. `chaos_db_outage_test.rs` — postgres unavailable during polling → workers retry; no tasks lost
3. `chaos_sigterm_test.rs` — SIGTERM during active execution → graceful drain; zero orphaned `Running` tasks
4. `chaos_max_retries_test.rs` — max retries exhausted → task transitions to `failed`; never re-queued

**Task handler registration:**
All task types MUST be registered before `engine.start()`. Registration is the
responsibility of the binary / integration point (`api` crate), never inside a task impl.

**Feature flags — `snake_case`, grouped by concern:**

```toml
[features]
default = []
tokio-console = ["dep:console-subscriber"]  # dev diagnostics only; never production
serde = ["dep:serde"]                       # optional for domain crate
```

### Figment Configuration Chain

The 6-step precedence order (documented in `crates/api/src/config.rs`):

1. **Compiled defaults** — `AppConfig::default()` in `crates/application/src/config.rs`
2. **Base config file** — `config.toml` at repo root (if present)
3. **Profile overlay** — `config.{IRON_DEFER_PROFILE}.toml` (e.g., `config.test.toml`)
4. **`.env` file** — loaded by dotenvy before figment
5. **Environment variables** — `IRON_DEFER__` prefix, `__` separator for nesting
6. **CLI flags** — always win (clap values override everything)

Higher-numbered sources override lower. The `IRON_DEFER__` prefix replaces the
original architecture's `APP__` prefix — changed during implementation for project
identity clarity.

### Arc Payload Pattern

`TaskRecord.payload` is `Arc<serde_json::Value>` rather than raw `serde_json::Value`.
This eliminates clone costs on the hot path (dispatch, retry, metrics recording).

```rust
// crates/domain/src/model/task.rs
pub(crate) payload: Arc<serde_json::Value>,

// Three accessors for different ownership needs:
pub fn payload(&self) -> &serde_json::Value    // borrow
pub fn payload_arc(&self) -> &Arc<serde_json::Value>  // shared ref
pub fn into_payload(self) -> Arc<serde_json::Value>   // move
pub fn take_payload(&mut self) -> Arc<serde_json::Value>  // replace with Null
```

At the HTTP boundary (`TryFrom<TaskRow>` in `postgres_task_repository.rs`), raw JSON is
wrapped in `Arc`. `Arc::unwrap_or_clone` unwraps without cloning when there's a single
owner (e.g., serializing the response body).

### Structured Error Types

Error types use discriminated enums for programmatic handling:

```rust
// crates/domain/src/error.rs
pub enum PayloadErrorKind { Validation { field, reason }, Deserialization { source }, ... }
pub enum ExecutionErrorKind { Timeout, HandlerPanic, Custom { source }, ... }

// TaskError variants reference these:
TaskError::InvalidPayload { kind: PayloadErrorKind }
TaskError::ExecutionFailed { kind: ExecutionErrorKind }
TaskError::Migration { source: Box<sqlx::migrate::MigrateError> }
```

### Error Payload Scrubbing

`scrub_database_message()` in `crates/infrastructure/src/error.rs` strips sensitive data
from sqlx error messages before they propagate through the adapter layer:

```rust
// crates/infrastructure/src/error.rs
fn scrub_database_message(msg: &str) -> String { ... }

pub(crate) enum PostgresAdapterError {
    Query { source: sqlx::Error },
    Configuration { message: String },
    DatabaseScrubbed { message: String, code: Option<String> },
    Mapping { reason: String },
}
```

The `From<sqlx::Error>` impl routes `sqlx::Error::Database` through
`scrub_database_message`, producing `DatabaseScrubbed` with the SQLSTATE code preserved
for programmatic use but payload-derived content removed.

### DispatchContext Pattern

`DispatchContext` in `crates/application/src/services/worker.rs` groups 8 dispatch-related
fields into a single cloneable struct, reducing the argument count on `dispatch_task()`:

```rust
#[derive(Clone)]
struct DispatchContext {
    repo: Arc<dyn TaskRepository>,
    registry: Arc<TaskRegistry>,
    worker_id: WorkerId,
    base_delay_secs: f64,
    max_delay_secs: f64,
    log_payload: bool,
    metrics: Option<Metrics>,
    queue_str: Arc<str>,
}
```

### OpenAPI / utoipa Integration

All HTTP handlers use `#[utoipa::path(...)]` macros for automatic OpenAPI schema generation.
The `OpenApi` derive on the API struct in `crates/api/src/http/router.rs` aggregates all
paths. The live spec is served at `GET /openapi.json`:

```rust
// crates/api/src/http/router.rs
#[derive(OpenApi)]
#[openapi(paths(...), components(schemas(...)))]
struct ApiDoc;

async fn openapi_spec() -> Json<utoipa::openapi::OpenApi> { ... }
```

### Jittered Backoff Formula

The retry backoff uses jitter to prevent thundering herd on mass failures:

```rust
next_delay = min(base_delay + if base_delay > 0 { random(0..base_delay) } else { 0 }, max_delay)
```

where `base_delay` doubles on each consecutive poll-loop error (not per-task failure),
capped at `max_delay`. This prevents workers from slamming the database simultaneously
when recovering from an outage. Backoff resets to the configured `base_delay` on
successful claim. Implementation: `crates/application/src/services/worker.rs`.

### Runtime-Typed `query_as` Pattern

For dynamic queries with optional filters (e.g., `list_tasks`), the engine uses
runtime-typed `sqlx::query_as` with `#[derive(sqlx::FromRow)]` on row types. This
avoids the limitations of the `query_as!` macro for optional WHERE clauses and
prevents `.sqlx/` cache bloat.

```rust
// crates/infrastructure/src/adapters/postgres_task_repository.rs
let mut query = sqlx::query_as::<_, TaskRowWithTotal>(&sql);
for param in params {
    query = query.bind(param);
}
let rows = query.fetch_all(&self.pool).await?;
```

This pattern is used in `list_tasks`, `queue_statistics`, and `cancel_task` to handle
dynamic SQL construction while maintaining the `TryFrom<TaskRow> for TaskRecord`
validation boundary.

### Claim-Cancellation Racing

Workers use `tokio::select!` between the claim attempt and the `CancellationToken`:

```rust
tokio::select! {
    _ = token.cancelled() => break,
    result = repo.claim_next(queue) => { ... }
}
```

Once a task is claimed, execution runs to completion — cancellation is never checked
mid-execution (see C2). If a claim is in flight when the token fires, the `select!`
resolves to the cancellation branch and the claimed task is released by the sweeper.

### Rust Engineering Standards

#### Newtype Pattern

**When to use:** Domain identifiers, units, and type-safe wrappers where the inner type
(usually `String`, `Uuid`, `i32`) would otherwise be interchangeable at call sites.

**How to implement:**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskKind(String);

impl TaskKind {
    pub fn as_str(&self) -> &str { &self.0 }
}
```

- Private inner field (no `pub` on the tuple element)
- `#[serde(transparent)]` for JSON round-trip compatibility
- Validation in `TryFrom` or a constructor — never raw `pub` field access
- Derive list: `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize` (omit `Hash`
  for types that are never map keys, e.g., `Priority`)
- Public accessor: `as_str()`, `as_uuid()`, `inner()`, or `Deref` — choose one per type

**Codebase references:** `TaskId`, `WorkerId`, `QueueName` in `crates/domain/src/model/`,
`TaskKind` in `kind.rs`, `AttemptCount`/`MaxAttempts` in `attempts.rs`, `Priority` in
`priority.rs`.

#### Builder Pattern & Context Structs

**When to use `bon::Builder`:** Structs with 4+ fields, especially domain models and service
constructors where optional vs. required parameters need compile-time enforcement.

**How to implement:**
```rust
#[derive(bon::Builder)]
pub struct TaskRecord {
    pub(crate) id: TaskId,
    pub(crate) queue: QueueName,
    // ... 12 more fields
}

// Services use #[builder] on new():
#[derive(bon::Builder)]
pub struct WorkerService { ... }
```

**When to use context structs instead:** Grouping related parameters for internal dispatch
where a full builder is overkill. Context structs are `#[derive(Clone)]` and passed by
reference:

```rust
#[derive(Clone)]
struct DispatchContext {
    repo: Arc<dyn TaskRepository>,
    registry: Arc<TaskRegistry>,
    // ... 6 more fields
}
```

**Codebase references:** `TaskRecord` in `crates/domain/src/model/task.rs` (`#[derive(bon::Builder)]`),
`WorkerService` in `crates/application/src/services/worker.rs` (`#[derive(bon::Builder)]`),
`DispatchContext` in `crates/application/src/services/worker.rs`.

#### Typestate / State Machine

**When to use:** Domain enums representing a state machine where forward compatibility matters.

**How to implement:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TaskStatus {
    Pending, Running, Completed, Failed, Cancelled,
}
```

- `#[non_exhaustive]` prevents downstream `match` from being exhaustive, enabling future
  variant additions without breaking changes
- Explicit match arms in the owning crate (no wildcards)
- `_ => Err(...)` or `_ => unreachable!()` in external crates only

**Codebase reference:** `TaskStatus` in `crates/domain/src/model/task.rs`.

#### Trait Ergonomics

**Object safety rules for `TaskHandler`:** No `Self: Sized` bounds; return
`Pin<Box<dyn Future>>` for async methods. This enables `Arc<dyn TaskHandler>` in the registry.

```rust
// crates/application/src/registry.rs — object-safe, no serde bounds
pub trait TaskHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>>;
}
```

**`Arc<dyn Trait>` vs generics decision:** Use `Arc<dyn Trait>` for shared ownership in
registries and runtime dispatch (handler lookup by `kind` string). Use generics for
compile-time dispatch in hot paths where the concrete type is known statically.

**Codebase references:** `TaskHandler` in `crates/application/src/registry.rs`,
`TaskHandlerAdapter<T>` in `crates/api/src/lib.rs` (bridges `impl Task` → `dyn TaskHandler`).

#### Private Fields with Typed Accessors

**When to use:** All domain structs — fields are `pub(crate)` or private, with public
accessor methods. Construction only through builders or validated constructors.

```rust
// crates/domain/src/model/task.rs
pub struct TaskRecord {
    pub(crate) id: TaskId,
    pub(crate) queue: QueueName,
    pub(crate) kind: TaskKind,
    pub(crate) payload: Arc<serde_json::Value>,
    // ... more fields
}

impl TaskRecord {
    pub fn id(&self) -> TaskId { self.id }
    pub fn queue(&self) -> &QueueName { &self.queue }
    pub fn kind(&self) -> &TaskKind { &self.kind }
    pub fn payload(&self) -> &serde_json::Value { &self.payload }
    // ... accessors return references for non-Copy types
}
```

- Copy types (`TaskId`, `TaskStatus`, `Priority`, `AttemptCount`, `MaxAttempts`, timestamps)
  return by value
- Non-Copy types (`QueueName`, `TaskKind`, `serde_json::Value`) return `&T`
- `Arc<serde_json::Value>` has three accessors: `payload()`, `payload_arc()`, `into_payload()`

**Codebase references:** `TaskRecord`, `TaskContext` in `crates/domain/src/model/task.rs`.

### Enforcement Guidelines

**All AI agents MUST:**
- Follow the crate module layout exactly — no new top-level modules without an ADR update
- Apply `#[instrument(skip(self), fields(...), err)]` on every public async method in
  `application` and `infrastructure` layers
- Return typed errors (never `Box<dyn Error>` in library code); convert at boundaries via `From` impls
- Use `camelCase` for JSON field names in REST responses (ADR-0006)
- Use the `iron_defer_` prefix for all OTel metric names
- Gate all dev-only tooling behind Cargo feature flags
- Never call `unwrap()` in library crates; use `expect("invariant: ...")` only where the
  invariant is documented inline
- `unwrap()` and `expect()` **are** permitted in `#[cfg(test)]` modules and integration test files

**Anti-patterns (FORBIDDEN):**
- `anyhow` in any library crate (permitted only in test utilities and one-off CLI scripts)
- `unwrap()` in `domain`, `application`, or `infrastructure` production code
- Global mutable state (`static mut`, `lazy_static` with interior mutability) — use `Arc<T>`
- Spawning a Tokio runtime inside a library function
- Logging the database URL or task payload without explicit config opt-in
- Constructing `TaskRegistry` outside the `api` crate
- Exposing infrastructure crate types in the public library API (except `PgPool` as input)

## Project Structure & Boundaries

### Complete Project Directory Structure

```
iron-defer/
├── Cargo.toml                          # workspace root — resolver = "2", all workspace deps
├── Cargo.lock                          # committed (binary + library)
├── CONTRIBUTING.md                     # contributor guide
├── rustfmt.toml                        # edition = "2024", max_width = 100
├── deny.toml                           # cargo deny — license, advisories, OpenSSL ban
├── .env.example                        # placeholder values; .env is gitignored
├── .gitignore
│
├── .cargo/
│   └── config.toml                     # build flags; [alias] for canonical commands:
│                                       #   check-all = "clippy --workspace --all-targets
│                                       #     -- -D clippy::pedantic"
│                                       #   test-all  = "test --workspace"
│
├── .sqlx/                              # COMMITTED — offline query cache (cargo sqlx prepare)
│   └── query-*.json                    # regenerate with: cargo sqlx prepare --workspace
│
├── .github/
│   └── workflows/
│       └── ci.yml                      # fmt → clippy → deny → migrations
│                                       #   → test → sqlx prepare --check
│
├── migrations/
│   ├── 0001_create_tasks_table.sql
│   ├── 0002_add_claim_check.sql        # claim-check index for SKIP LOCKED
│   └── 0003_add_pagination_index.sql   # pagination index for GET /tasks
│
├── docker/
│   ├── Dockerfile                      # multi-stage: rust:1.94-slim → distroless/cc
│   │                                   #   builder stage requires .sqlx/ (SQLX_OFFLINE=true)
│   ├── docker-compose.yml              # standalone iron-defer + postgres
│   ├── docker-compose.dev.yml          # postgres-only for embedded library dev
│   └── smoke-test.sh                   # Docker deployment smoke test
│
├── k8s/
│   ├── kustomization.yaml              # references deployment + configmap + service
│   ├── deployment.yaml                 # terminationGracePeriodSeconds: 60
│   ├── configmap.yaml                  # IRON_DEFER__ env var defaults
│   └── service.yaml                    # ClusterIP on REST API port
│
├── docs/
│   ├── adr/
│   │   ├── 0001-hexagonal-architecture.md
│   │   ├── 0002-error-handling.md
│   │   ├── 0003-configuration-management.md
│   │   ├── 0004-async-runtime-tokio-ecosystem.md
│   │   ├── 0005-database-layer-sqlx.md
│   │   └── 0006-serialization-serde.md
│   └── guidelines/
│       ├── rust-idioms.md
│       ├── quality-gates.md
│       ├── compliance-evidence.md      # regulatory compliance evidence mapping
│       ├── postgres-reconnection.md    # pool recovery and reconnection strategy
│       ├── security.md                 # security posture and payload privacy
│       └── structured-logging.md       # structured logging conventions
│
└── crates/
    ├── domain/
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # re-exports only
    │       ├── model/
    │       │   ├── mod.rs
    │       │   ├── task.rs             # TaskRecord (#[derive(bon::Builder)]), TaskStatus
    │       │   │                       #   (#[non_exhaustive]), TaskContext, CancelResult,
    │       │   │                       #   ListTasksFilter
    │       │   ├── worker.rs           # WorkerId
    │       │   ├── queue.rs            # QueueName newtype
    │       │   ├── kind.rs             # TaskKind newtype
    │       │   ├── attempts.rs         # AttemptCount, MaxAttempts newtypes
    │       │   └── priority.rs         # Priority newtype
    │       └── error.rs                # TaskError, PayloadErrorKind, ExecutionErrorKind,
    │                                   #   ValidationError
    │
    ├── application/
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # re-exports only
    │       ├── config.rs               # AppConfig, WorkerConfig, DatabaseConfig,
    │       │                           #   ServerConfig, ObservabilityConfig
    │       ├── metrics.rs              # OTel metric instrument handles (Metrics struct)
    │       ├── ports/
    │       │   ├── mod.rs
    │       │   ├── task_repository.rs  # TaskRepository trait
    │       │   └── task_executor.rs    # TaskExecutor trait
    │       ├── registry.rs             # TaskHandler trait + TaskRegistry
    │       └── services/
    │           ├── mod.rs
    │           ├── scheduler.rs        # SchedulerService: enqueue, cancel, inspect
    │           ├── worker.rs           # WorkerService (pool) + DispatchContext
    │           └── sweeper.rs          # SweeperService (independent of worker pool)
    │
    ├── infrastructure/
    │   ├── Cargo.toml
    │   ├── src/                        # ← src/ and tests/ are siblings
    │   │   ├── lib.rs                  # re-exports only
    │   │   ├── db.rs                   # create_pool() — PgPoolOptions wiring
    │   │   ├── error.rs                # PostgresAdapterError (Query, Configuration,
    │   │   │                           #   DatabaseScrubbed, Mapping) + scrub_database_message
    │   │   ├── adapters/
    │   │   │   ├── mod.rs
    │   │   │   └── postgres_task_repository.rs
    │   │   └── observability/
    │   │       ├── mod.rs
    │   │       ├── metrics.rs          # OTel meter + all iron_defer_* instruments
    │   │       └── tracing.rs          # tracing-subscriber + JSON formatter init
    │   └── tests/                      # ← sibling of src/ (Rust integration test convention)
    │       ├── common/
    │       │   └── mod.rs              # TEST_DB OnceCell + test_pool() +
    │       │                           #   fresh_pool_on_shared_container()
    │       ├── task_repository_test.rs # save, find, claim, list, pagination
    │       ├── init_tracing_test.rs    # tracing init verification
    │       └── tracing_privacy_test.rs # payload privacy in log output
    │
    └── api/
        ├── Cargo.toml                  # [lib] iron_defer
        │                               # [[bin]] iron-defer
        │                               # [[example]] basic_enqueue, axum_integration
        │                               # [[bench]] throughput
        ├── src/                        # ← src/, examples/, benches/, tests/ are siblings
        │   ├── lib.rs                  # IronDefer builder — embedded library façade
        │   │                           #   (IS implementation; not re-exports-only)
        │   ├── main.rs                 # standalone binary entry point
        │   ├── cli/                    # clap CLI (modular)
        │   │   ├── mod.rs              # Cli enum + Serve subcommand
        │   │   ├── submit.rs           # task submission commands
        │   │   ├── tasks.rs            # task inspection commands
        │   │   ├── workers.rs          # worker management commands
        │   │   ├── db.rs               # database commands (migrate, etc.)
        │   │   ├── config.rs           # config display commands
        │   │   └── output.rs           # output formatting helpers
        │   ├── config.rs               # figment chain: defaults→file→profile→.env→env→cli
        │   ├── shutdown.rs             # CancellationToken root, OS signals, drain timeout
        │   └── http/
        │       ├── mod.rs
        │       ├── router.rs           # axum Router + middleware + OpenAPI spec (/openapi.json)
        │       ├── extractors.rs       # custom axum extractors
        │       ├── handlers/
        │       │   ├── mod.rs
        │       │   ├── tasks.rs        # POST /tasks, GET /tasks/{id},
        │       │   │                   #   DELETE /tasks/{id}, GET /tasks
        │       │   ├── health.rs       # GET /health/live, GET /health/ready
        │       │   ├── queues.rs       # GET /queues/{queue}/stats
        │       │   └── metrics.rs      # GET /metrics (Prometheus scrape endpoint)
        │       └── errors.rs           # AppError + ErrorResponse + IntoResponse
        ├── examples/                   # registered [[example]] targets — compiled + type-checked
        │   ├── basic_enqueue.rs        # cargo run --example basic_enqueue
        │   └── axum_integration.rs     # cargo run --example axum_integration
        ├── benches/                    # criterion benchmarks — [[bench]] in Cargo.toml
        │   └── throughput.rs           # verifies ≥10,000 jobs/sec NFR
        └── tests/                      # ← sibling of src/ (Rust integration test convention)
            ├── common/
            │   ├── mod.rs              # full-stack test setup (shared OnceCell)
            │   └── otel.rs             # OTel test helpers (meter provider, collector mock)
            ├── chaos_common.rs         # shared helpers for chaos tests
            ├── integration_test.rs     # submit → claim → complete round-trip
            ├── rest_api_test.rs        # REST API endpoint tests
            ├── cli_test.rs             # CLI command tests
            ├── config_validation_test.rs # config parsing and validation
            ├── audit_trail_test.rs     # audit trail compliance tests
            ├── lifecycle_log_test.rs   # task lifecycle structured logging
            ├── observability_test.rs   # observability integration
            ├── metrics_test.rs         # OTel metrics recording
            ├── otel_counters_test.rs   # OTel counter instruments
            ├── otel_lifecycle_test.rs  # OTel lifecycle events
            ├── otel_metrics_test.rs    # OTel metrics compliance
            ├── pool_exhaustion_test.rs # connection pool exhaustion handling
            ├── shutdown_test.rs        # graceful shutdown verification
            ├── sweeper_test.rs         # sweeper zombie recovery
            ├── sweeper_counter_test.rs # sweeper metrics counters
            ├── worker_pool_test.rs     # worker pool concurrency
            ├── chaos_db_outage_test.rs   # postgres down → no task loss
            ├── chaos_max_retries_test.rs # exhausted retries → failed, not pending
            ├── chaos_sigterm_test.rs     # SIGTERM → zero orphaned Running tasks
            └── chaos_worker_crash_test.rs # kill worker mid-execution → sweeper recovers
```

### Architectural Boundaries

**Layer dependency rules (enforced by Cargo crate boundaries):**

```
domain         ← no dependencies on other workspace crates
application    ← domain only
infrastructure ← domain + application + all external crates
api            ← all crates (wiring only)
```

Violations detectable via `cargo tree`, blocked in code review.

**Public library API boundary (`crates/api/src/lib.rs`):**
- Exposes: `IronDefer`, `IronDeferBuilder`, `Task` trait, `TaskContext`, `TaskError`
- Accepts as input: `sqlx::PgPool` (sole infrastructure type crossing the public boundary)
- Never exposes: `axum`, `reqwest`, or other `sqlx` types

**REST API boundary:**
- Inbound: validated at handler boundary via request structs; business rules checked before
  entering service layer
- Outbound: domain types mapped to response DTOs at handler; errors converted to `AppError` JSON
- Body size limit: 1 MiB (`DefaultBodyLimit`)

**Database boundary:**
- `TaskRow` types are `pub(crate)` — never cross the infrastructure crate boundary
- Domain types constructed via `TryFrom<TaskRow>` in the adapter
- All queries compile-time verified via `.sqlx/` offline cache (`SQLX_OFFLINE=true` in CI/Docker)

**Chaos test isolation boundary:**
- `crates/api/tests/chaos_*.rs` tests must each spin up their own Postgres container
- Must NOT use the shared `TEST_DB OnceCell` — chaos tests kill connections and corrupt
  shared state for concurrent tests
- Shared chaos setup lives in `crates/api/tests/chaos_common.rs`

### Requirements to Structure Mapping

| Functional Requirement | Primary File(s) |
|---|---|
| Submit task (POST /tasks) | `api/http/handlers/tasks.rs` → `application/services/scheduler.rs` |
| Get task status (GET /tasks/{id}) | `api/http/handlers/tasks.rs` → `application/services/scheduler.rs` |
| Cancel task (DELETE /tasks/{id}) | `api/http/handlers/tasks.rs` → `application/services/scheduler.rs` |
| List / inspect queue (GET /tasks) | `api/http/handlers/tasks.rs` → `application/services/scheduler.rs` |
| Queue statistics (GET /queues) | `api/http/handlers/queues.rs` → `application/services/scheduler.rs` |
| Atomic claim (SKIP LOCKED) | `infrastructure/adapters/postgres_task_repository.rs` |
| Worker pool + poll loop | `application/services/worker.rs` |
| Sweeper / zombie recovery | `application/services/sweeper.rs` |
| Exponential backoff retry | `application/services/worker.rs` (jittered backoff formula) |
| SIGTERM graceful drain | `api/src/shutdown.rs` |
| OTel metrics | `application/metrics.rs` + `infrastructure/observability/metrics.rs` |
| Structured logging | `infrastructure/observability/tracing.rs` |
| Prometheus scrape endpoint | `api/http/handlers/metrics.rs` |
| OpenAPI spec endpoint | `api/http/router.rs` (GET /openapi.json via utoipa) |
| Embedded library façade | `api/src/lib.rs` |
| Standalone binary | `api/src/main.rs` |
| CLI interface | `api/src/cli/` (modular: submit, tasks, workers, db, config, output) |
| Postgres schema | `migrations/` (3 files: create table, claim check, pagination index) |
| Throughput NFR (≥10k jobs/sec) | `crates/api/benches/throughput.rs` (criterion) |
| Docker deployment | `docker/Dockerfile` + `docker/docker-compose.yml` + `docker/smoke-test.sh` |
| Kubernetes deployment | `k8s/` (kustomize) |
| Health endpoint | `api/http/handlers/health.rs` (liveness + readiness probes) |

### Integration Points

**Enqueue data flow:**
```
HTTP POST /tasks
  → axum handler (tasks.rs) → CreateTaskCommand validation
  → SchedulerService::enqueue() (application)
  → TaskRepository::save() port
  → PostgresTaskRepository INSERT INTO tasks (infrastructure)
```

**Execution data flow:**
```
WorkerService poll loop (application)
  → TaskRepository::claim_next() — atomic UPDATE SKIP LOCKED (infrastructure)
  → TaskRegistry::dispatch(kind) → Arc<dyn TaskHandler>
  → TaskHandler::execute(payload, ctx)
  → TaskRepository::complete() or ::fail()
  → OtelMetrics::record_duration() (infrastructure/observability)
```

**External integrations:**
- PostgreSQL: sole external dependency; `PgPool` via `infrastructure/db.rs`
- OTel Collector: OTLP/gRPC from `infrastructure/observability/metrics.rs`
- Docker registry: release pipeline on tag
- No other external service dependencies in MVP

### Development Workflow

**Local development:**
```bash
docker compose -f docker/docker-compose.dev.yml up -d   # postgres only
sqlx migrate run
cargo test-all                                           # alias: .cargo/config.toml
```

**CI pipeline order (`.github/workflows/ci.yml`):**

The CI job runs on `ubuntu-latest` with a `postgres:16-alpine` service container and
`SQLX_OFFLINE=true` set globally.

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                             # bans + advisories + licenses
sqlx database setup                          # run migrations on service DB
cargo test --workspace
cargo sqlx prepare --check --workspace       # verify .sqlx/ matches current queries
```

Note: `cargo audit`, `cargo machete`, and `cargo tarpaulin` are not in the current
CI pipeline. Throughput benchmarks run locally — no `release.yml` exists.

**Docker build:**
Multi-stage: `rust:1.94-slim` builder → `gcr.io/distroless/cc` runtime.
Builder stage sets `SQLX_OFFLINE=true` and copies `.sqlx/` — no live DB needed at build time.
`docker/smoke-test.sh` validates the built image starts correctly and responds to health checks.

**Kubernetes:**
`terminationGracePeriodSeconds: 60` (≥ shutdown_timeout default 30s).
ConfigMap uses `IRON_DEFER__` env var prefix (not `APP__`).
Apply: `kubectl apply -k k8s/`

## Architecture Validation Results

### Coherence Validation ✓

**Decision compatibility:** All technology choices are mutually compatible.
Rust 2024 / MSRV 1.94, Tokio ecosystem, SQLx 0.8, opentelemetry 0.27, rustls — no conflicts.

**Important:** Pin the opentelemetry crate family versions exactly in workspace `Cargo.toml`.
`opentelemetry`, `opentelemetry-otlp`, and `tracing-opentelemetry` must be on matching versions
to prevent minor-version drift breaking the build.

**Pattern consistency:** All naming conventions, error propagation rules, tracing patterns,
and JSON serialization conventions are internally consistent and aligned with ADRs 0001–0006.
No contradictions found.

**Structure alignment:** 4-crate workspace matches hexagonal layer rules exactly. `tests/`
directories are siblings of `src/`. `.sqlx/` at workspace root matches `sqlx::migrate!`
path in ADR-0005. ✓

### Requirements Coverage Validation ✓

**Functional requirements: 17/17 covered.**
All FR categories (Task Lifecycle, Distributed Execution, Resilience, Observability,
Dual Deployment, CLI) have explicit architectural support and named implementation files.

**Non-functional requirements: 10/10 covered.**
Every NFR has both an architectural mechanism and a verification path.

| NFR | Mechanism | Verification Path |
|-----|-----------|-------------------|
| ≥10,000 jobs/sec | SKIP LOCKED single-query claim | `benches/throughput.rs` (criterion) |
| Recovery < lease duration | SweeperService | `chaos/worker_crash_test.rs` |
| Zero task loss | At-least-once + chaos suite | All 4 chaos tests |
| Time-to-first-task < 30min | `lib.rs` builder API + examples | `examples/basic_enqueue.rs` |
| OTel metrics + logs | `observability/` | Integration test vs OTel Collector |
| At-least-once guarantee | SKIP LOCKED + sweeper | `chaos/worker_crash_test.rs` |
| Graceful shutdown | `shutdown.rs` + `CancellationToken` | `chaos/sigterm_test.rs` |
| PostgreSQL 14+ | Dependency constraint | CI matrix |
| Rust MSRV 1.94 | `workspace.package.rust-version` | `cargo check` on MSRV toolchain |
| No new infrastructure | Design constraint | Architecture self-enforcing |

**Compliance / regulatory: 7/7 frameworks covered.**
PCI DSS, GDPR, HIPAA, DORA, NIS2, SOC 2 CC7.2, ISO 27001:2022 supported through:
tasks table audit trail, rustls TLS, structured logging with task_id correlation,
OTel metrics, cargo deny/audit supply chain governance, on-premises deployment model.

### Implementation Readiness Validation ✓

**Decision completeness:** All critical decisions documented with versions and rationale.
9 consistency conflict points resolved. 4 mandatory enforcement rules + 7 anti-patterns
specified. Three party mode sessions added 24 cumulative improvements.

**Structure completeness:** Every file and directory named. Integration data flows mapped.
Chaos test isolation boundary documented. `.sqlx/` cache lifecycle specified end-to-end.

**Pattern completeness:** Naming, error handling, tracing, testcontainers sharing, chaos test
isolation, task registration ownership, feature flags, and public API boundary — all specified
with examples and anti-patterns.

### Critical Implementation Clarifications

The following were identified in final validation and must be followed by all implementing agents:

**C1 — axum graceful shutdown requires explicit wiring (HIGH):**
`axum::serve(...)` does NOT stop on process signal automatically. Both `main.rs` and
`lib.rs` must wire shutdown explicitly:
```rust
axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_token.cancelled())
    .await?;
```
Without this, the HTTP server keeps accepting connections after the drain timeout fires.

**C2 — CancellationToken polled BETWEEN tasks only, never mid-execution (HIGH):**
The token is checked at the top of the worker poll loop — racing the claim attempt via
`tokio::select!`. Once a task is claimed and execution begins, it runs to completion
(or explicit failure). NEVER wrap task execution in `tokio::select!` against the
cancellation token. Cancelling mid-execution creates zombie tasks — exactly what the
Sweeper exists to recover.

```rust
// CORRECT — race cancellation against claim, not execution
loop {
    tokio::select! {
        _ = token.cancelled() => break,
        result = repo.claim_next(queue) => {
            if let Some(task) = result? {
                execute_to_completion(task).await; // no cancellation here
            }
        }
    }
}
```

If a claim is in flight when the token fires, `select!` resolves to the cancellation
branch. The possibly-claimed task (if the DB already committed) is recovered by the
sweeper on its next cycle — this is the designed safety net.

**C3 — Embedded library migration strategy (HIGH):**
The embedded `IronDefer` library must embed migrations at compile time using
`sqlx::migrate!()` with no path argument (defaults to `./migrations` relative to crate root,
which is wrong for a library). Use the `include_migrations!` pattern or embed via:
```rust
// In crates/api/src/lib.rs
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
```
This embeds migrations into the library binary at compile time.
`.skip_migrations(true)` opt-out is for callers who manage migrations externally.
Expose `IronDefer::migrator() -> &'static Migrator` so callers can inspect or run
migrations in their own transaction if needed.

**C4 — TaskHandler object-safe erased trait pattern (HIGH):**
The registry stores `Arc<dyn TaskHandler>`. `TaskHandler` is defined in
`crates/application/src/registry.rs`:

```rust
// application/src/registry.rs — object-safe, no serde bounds
pub trait TaskHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>>;
}
```

The adapter that bridges `impl Task` → `Arc<dyn TaskHandler>` lives in `crates/api/src/lib.rs`
(needs serde bounds — not in application):

```rust
// api/src/lib.rs — registration adapter
struct TaskHandlerAdapter<T: Task>(PhantomData<T>);

impl<T: Task> TaskHandler for TaskHandlerAdapter<T> {
    fn kind(&self) -> &'static str { T::KIND }
    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
        Box::pin(async move {
            let task: T = serde_json::from_value(payload.clone())
                .map_err(|e| TaskError::InvalidPayload { reason: e.to_string() })?;
            task.execute(ctx).await
        })
    }
}

// IronDeferBuilder::register<T: Task>() creates Arc<TaskHandlerAdapter<T>>
```

**C5 — POST /tasks queue field: body field, not path parameter (MEDIUM):**
Use a single `POST /tasks` endpoint. The `queue` field is optional in the request body,
defaulting to `"default"`. Drop the `/queues/{queue}/tasks` route — it is redundant.

```json
// POST /tasks
{ "kind": "PaymentWebhook", "payload": {...}, "queue": "default", "priority": 0,
  "scheduledAt": "2026-04-04T12:00:00Z" }
```

**C6 — Application layer mocks: use `mockall` (MEDIUM):**
Port traits in `crates/application/src/ports/` must be annotated with `#[automock]`
(from `mockall`). Add `mockall` to `[dev-dependencies]` in `crates/application/Cargo.toml`.

```rust
// application/src/ports/task_repository.rs
#[cfg_attr(test, mockall::automock)]
pub trait TaskRepository: Send + Sync {
    async fn save(&self, task: &Task) -> Result<(), TaskError>;
    // ...
}
```

Unit tests in `application` and `api` crates use `MockTaskRepository`, `MockTaskExecutor`.
Never use real database connections in application-layer unit tests.

**C7 — Throughput benchmarks require external DATABASE_URL; run locally (MEDIUM):**
Criterion benchmarks in `crates/api/benches/throughput.rs` require a real Postgres instance.
They are NOT run in CI (`ci.yml`). No `release.yml` exists — benchmarks are run locally
by the developer against a local or Docker Compose Postgres instance.

```bash
# Local benchmark run
DATABASE_URL=postgres://postgres@localhost/iron_defer_bench \
    cargo bench --bench throughput
```

Document `DATABASE_URL` as a required environment variable in the benchmark file header.

### Gap Analysis Results

**Critical gaps:** None. All implementation-blocking decisions are made and clarified.

**Important gaps (2 — documented, non-blocking):**

1. **Health endpoint response:** Two endpoints:
   - `GET /health/live` → liveness: `200 {}`
   - `GET /health/ready` → readiness: checks DB pool; `200 {"status":"ready","db":"ok"}`
     or `503 {"status":"degraded","db":"unavailable"}`

2. **CLI output format:** Human-readable default; `--json` flag for machine-readable.
   Exit codes: `0` success, `1` application error, `2` usage/argument error.

**Deferred (Growth/Vision):**
- Helm chart (currently Kustomize only)
- LISTEN/NOTIFY fast-path polling
- `cargo audit`, `cargo machete`, `cargo tarpaulin` CI gates (not yet in `ci.yml`)
- `release.yml` workflow for Docker build + push on tag

### Architecture Completeness Checklist

- [x] Project context — 17 FRs, 10 NFRs, 7 compliance frameworks
- [x] Workspace initialization — Rust 2024, MSRV 1.94, rustls-only, edition 2024
- [x] Data architecture — tasks table, SKIP LOCKED claim, retry formula, retention policy
- [x] Concurrency model — JoinSet + Semaphore + interval polling
- [x] Sweeper design — independent tokio task, two-query recovery
- [x] Security architecture — no-auth MVP boundary, TLS, payload privacy
- [x] OTel integration — metric names, types, labels, SDK wiring
- [x] Graceful shutdown — CancellationToken tree, axum explicit wiring, drain timeout
- [x] Public library API — IronDefer builder, Task trait, TaskHandler erased dispatch
- [x] Naming conventions — DB, API, Rust identifiers, OTel
- [x] Module structure — per-crate layout, lib.rs scope, no-logic rule scope
- [x] REST formats — direct responses, error shape, status codes, date format
- [x] Process patterns — registry ownership, shutdown responsibilities, mock strategy
- [x] Complete directory tree — all files including .sqlx/, benches/, examples/
- [x] CI pipeline — 6 steps (fmt, clippy, deny, migrate, test, sqlx prepare --check)
- [x] Docker — multi-stage, SQLX_OFFLINE=true, distroless/cc runtime; smoke test script
- [x] Kubernetes — kustomize, terminationGracePeriodSeconds: 60
- [x] Requirements-to-structure mapping — all 19 items mapped

### Architecture Readiness Assessment

**Overall Status: READY FOR IMPLEMENTATION**

**Confidence level: High**

**Key strengths:**
- Pre-committed ADR stack eliminates technology ambiguity for all implementing agents
- Single atomic SKIP LOCKED claim is the correctness linchpin — fully specified
- Hexagonal crate boundaries make layer violations compile-time detectable
- Chaos test manifest gives at-least-once guarantee a verifiable acceptance path
- All NFRs have both an architectural mechanism and a named verification path
- Three party mode reviews added 24 total improvements across all sections
- Critical implementation clarifications (C1–C7) prevent the most dangerous agent mistakes

**Areas for future enhancement (Growth phase):**
- LISTEN/NOTIFY for sub-100ms task pickup latency
- Transactional enqueue (River pattern) for dual-write elimination
- Full OTel 4-pillar coverage (traces + events + W3C propagation)
- REST API authentication (bearer token / mTLS / OIDC)
- Append-only `task_history` table for tamper-evident audit log
- Geographic worker pinning for GDPR/HIPAA data residency routing
- Exactly-once semantics with idempotency keys
- Checkpoint/resume for multi-step workflows

### Implementation Handoff

**AI agent guidelines:**
- All architectural decisions are final — implement exactly as documented
- Critical clarifications C1–C7 are mandatory — not optional guidance
- Respect crate layer boundaries — `cargo tree` violations are blocking in review
- Apply `#[instrument(skip(self), fields(...), err)]` on every public async method
- Use `MockTaskRepository` / `MockTaskExecutor` (mockall) for application unit tests
- Chaos tests use isolated per-test containers — never the shared `OnceCell`
- Refer to this document for all architectural questions before making local decisions
- The PRD and ADRs 0001–0006 are the companion references

**First implementation story:** Cargo workspace initialization
- Create workspace `Cargo.toml` (resolver = "2", edition = "2024", rust-version = "1.94")
- Scaffold 4 crates: domain, application, infrastructure, api
- Add dual-target to `api/Cargo.toml` (lib + bin + examples + benches)
- Configure `rustfmt.toml`, `deny.toml`, `.cargo/config.toml`
- First commit: empty workspace compiling cleanly with `cargo check-all` passing

**Implementation sequence:**
1. Workspace initialization + scaffolding
2. Domain model (`Task`, `TaskId`, `WorkerId`, `TaskStatus`, error types)
3. Application ports + `TaskRegistry` + `TaskHandler` erased trait
4. Postgres migration + `PostgresTaskRepository`
5. Claiming engine (atomic SKIP LOCKED query)
6. Worker pool (`JoinSet` + `Semaphore` + poll loop with `CancellationToken`)
7. Sweeper task
8. OTel metrics integration
9. REST API (axum) + CLI (clap)
10. Graceful shutdown (`CancellationToken` wiring + axum `with_graceful_shutdown`)
11. Embedded library façade (`IronDefer` builder + migration embedding)
12. Standalone binary wiring
13. Chaos integration tests (isolated containers per test)
14. Throughput benchmark (criterion, local-only)

---

## Growth Phase Architecture Addendum

*Added 2026-04-24. Covers features G1-G8 from PRD §Growth Features. MVP architecture above remains authoritative for all existing components.*

### Schema Evolution

All Growth features use additive schema changes — no existing columns are modified or removed. Migrations are numbered sequentially after the MVP migration set.

#### New Columns on `tasks` Table

```sql
-- G1: Idempotency keys
ALTER TABLE tasks ADD COLUMN idempotency_key VARCHAR;
ALTER TABLE tasks ADD COLUMN idempotency_expires_at TIMESTAMPTZ;
CREATE UNIQUE INDEX idx_tasks_idempotency
    ON tasks (queue, idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status NOT IN ('completed', 'failed', 'cancelled');

-- G4: Trace context
ALTER TABLE tasks ADD COLUMN trace_id VARCHAR;

-- G6: Checkpoint/resume
ALTER TABLE tasks ADD COLUMN checkpoint JSONB;

-- G7: HITL suspend/resume
ALTER TABLE tasks ADD COLUMN signal_payload JSONB;
ALTER TABLE tasks ADD COLUMN suspended_at TIMESTAMPTZ;

-- G8: Geographic worker pinning
ALTER TABLE tasks ADD COLUMN region VARCHAR;
CREATE INDEX idx_tasks_region_claiming
    ON tasks (queue, region, status, priority DESC, scheduled_at ASC)
    WHERE status = 'pending';
```

#### New Table: `task_audit_log` (G5)

```sql
CREATE TABLE task_audit_log (
    id          BIGSERIAL PRIMARY KEY,
    task_id     UUID NOT NULL REFERENCES tasks(id),
    from_status TEXT,
    to_status   TEXT NOT NULL,
    timestamp   TIMESTAMPTZ NOT NULL DEFAULT now(),
    worker_id   UUID,
    trace_id    VARCHAR,
    metadata    JSONB
);

CREATE INDEX idx_audit_log_task_time
    ON task_audit_log (task_id, timestamp);

-- Immutability enforcement (NFR-C1)
CREATE OR REPLACE FUNCTION audit_log_immutable()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'audit log is append-only: % operations are forbidden',
        TG_OP;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_audit_log_immutable
    BEFORE UPDATE OR DELETE ON task_audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_immutable();
```

#### Conditional Table Creation (G3: UNLOGGED)

When `database.unlogged_tables: true`, the migration uses `CREATE UNLOGGED TABLE tasks (...)` instead of `CREATE TABLE tasks (...)`. This is implemented as a conditional migration — the migration runner checks the config flag and selects the appropriate DDL. The `task_audit_log` table is always `LOGGED` regardless of this flag (mutual exclusion enforced at startup per FR40).

### TaskStatus Enum Extension

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Suspended,  // G7: HITL suspend/resume
}
```

The `Suspended` variant is the only enum addition. `#[non_exhaustive]` is already in place (Epic 6), so downstream `match` arms with `_ =>` wildcards continue to compile without changes.

**Status transition rules for `Suspended`:**
- `Running → Suspended`: via `ctx.suspend()` in the task handler
- `Suspended → Pending`: via `POST /tasks/{id}/signal` (external signal)
- `Suspended → Failed`: via suspend watchdog timeout (Sweeper)
- All other transitions from `Suspended` are invalid (409 TASK_NOT_IN_EXPECTED_STATE)

### TaskContext Extension

```rust
pub struct TaskContext {
    // Existing (MVP)
    pub(crate) task_id: TaskId,
    pub(crate) worker_id: WorkerId,
    pub(crate) attempt: AttemptCount,

    // Growth additions
    pub(crate) trace_context: Option<opentelemetry::Context>,  // G4
    pub(crate) last_checkpoint: Option<serde_json::Value>,      // G6
    pub(crate) signal_payload: Option<serde_json::Value>,       // G7
    pub(crate) pool: PgPool,                                    // G6: for checkpoint writes
}

impl TaskContext {
    // Existing accessors unchanged

    // G4: Trace context
    pub fn trace_context(&self) -> Option<&opentelemetry::Context> { ... }

    // G6: Checkpoint/resume
    pub async fn checkpoint(&self, data: serde_json::Value) -> Result<(), TaskError> {
        // UPDATE tasks SET checkpoint = $1 WHERE id = $2
        // Single DB round-trip, bounded by 1 MiB payload limit
    }
    pub fn last_checkpoint(&self) -> Option<&serde_json::Value> { ... }

    // G7: HITL suspend/resume
    pub async fn suspend(&self) -> Result<(), TaskError> {
        // 1. Persist checkpoint (required — G6 dependency)
        // 2. UPDATE tasks SET status = 'suspended', suspended_at = now()
        // 3. Release SKIP LOCKED advisory lock
        // 4. Release DB connection
        // 5. Return SuspendSignal to worker loop (new internal type)
    }
    pub fn signal_payload(&self) -> Option<&serde_json::Value> { ... }
}
```

**Critical constraint:** `ctx.suspend()` must checkpoint before releasing the worker slot. The worker loop receives a `SuspendSignal` (new internal enum variant in the dispatch result) and returns the slot to the pool without marking the task as failed.

### New API Surface

#### Embedded Library API

```rust
impl IronDefer {
    // G1: Idempotency
    pub async fn enqueue_idempotent<T: Task>(
        &self, queue: &str, task: T, idempotency_key: &str
    ) -> Result<(TaskRecord, bool), TaskError> { ... }
    // Returns (record, created) — created=false for duplicate key

    // G2: Transactional enqueue
    pub async fn enqueue_in_tx<'a, T: Task>(
        &self, tx: &mut sqlx::Transaction<'a, Postgres>, queue: &str, task: T
    ) -> Result<TaskRecord, TaskError> { ... }

    // G8: Geographic pinning
    pub async fn enqueue_with_region<T: Task>(
        &self, queue: &str, task: T, region: &str
    ) -> Result<TaskRecord, TaskError> { ... }
}
```

#### REST API Additions

| Method | Endpoint | Feature | Description |
|--------|----------|---------|-------------|
| POST | `/tasks` | G1 | New optional `idempotencyKey` field in request body. Returns 200 (not 201) for duplicate key |
| POST | `/tasks/{id}/signal` | G7 | Resume a suspended task with optional JSON payload. Returns 200 or 409 |
| GET | `/tasks/{id}` | G6 | Response gains `lastCheckpoint` field (nullable) |
| GET | `/tasks/{id}` | G7 | Response gains `suspendedAt` field (nullable) |
| GET | `/tasks/{id}` | G8 | Response gains `region` field (nullable) |

**No existing endpoints change behavior.** New fields are additive (nullable) in responses.

### Sweeper Modifications

The Sweeper gains two new responsibilities piggybacked on its existing tick:

```
Sweeper tick (every sweeper_interval):
  1. [MVP]    Recover zombie tasks: Running + claimed_until < now()
  2. [G1]     Clean expired idempotency keys:
              DELETE FROM tasks WHERE idempotency_expires_at < now()
              AND status IN ('completed', 'failed', 'cancelled')
              AND idempotency_key IS NOT NULL
  3. [G7]     Suspend watchdog: auto-fail tasks suspended too long:
              UPDATE tasks SET status = 'failed',
                  last_error = 'suspend timeout exceeded'
              WHERE status = 'suspended'
                AND suspended_at < now() - suspend_timeout
```

The Sweeper explicitly **skips** `Suspended` tasks in the zombie recovery query (clause 1). The `WHERE status = 'running'` predicate in the existing zombie query naturally excludes them.

### Claiming Engine Modifications

#### G8: Geographic Pinning

The `SKIP LOCKED` claiming query gains a region predicate:

```sql
-- Worker WITH region configured:
SELECT * FROM tasks
WHERE queue = $1
  AND status = 'pending'
  AND scheduled_at <= now()
  AND (region IS NULL OR region = $worker_region)
ORDER BY priority DESC, scheduled_at ASC
LIMIT 1
FOR UPDATE SKIP LOCKED;

-- Worker WITHOUT region configured:
SELECT * FROM tasks
WHERE queue = $1
  AND status = 'pending'
  AND scheduled_at <= now()
  AND region IS NULL
ORDER BY priority DESC, scheduled_at ASC
LIMIT 1
FOR UPDATE SKIP LOCKED;
```

Regionless workers claim only unpinned tasks. Workers with a region claim both matching and unpinned tasks.

### OTel Trace Integration (G4)

#### Span Architecture

```
enqueue(traceparent) → [store trace_id in tasks row]
                              │
worker claims task ──────────►│
                              ▼
                    create child span {
                        trace_id: from tasks.trace_id
                        span_id: fresh
                        attributes: task_id, queue, kind, attempt
                    }
                              │
                    execute handler ──► span ends
                              │
                    record task_duration_seconds with span context
```

**Trace context storage:** The `trace_id` column on the tasks table persists the W3C trace ID across the enqueue→claim boundary. The worker reads it when claiming and creates a child span. If no `traceparent` was supplied at enqueue time, `trace_id` is NULL and no span is created (backward-compatible).

**OTel Events:** Each state transition emits an OTel Event (Log Record) with attributes: `task_id`, `from_status`, `to_status`, `queue`, `kind`, `worker_id`. These supplement — not replace — the existing `tracing` structured logs.

#### Test Infrastructure

Trace propagation tests use an **in-memory span exporter** (`opentelemetry_sdk::testing::InMemorySpanExporter`) rather than a full collector testcontainer. This avoids the infrastructure cost flagged in the party mode review while still verifying trace ID propagation.

### Audit Log Integration (G5)

#### Write Path

Audit log inserts happen inside the **same database transaction** as the state transition:

```rust
// Inside state_transition():
sqlx::query("UPDATE tasks SET status = $1 ... WHERE id = $2")
    .execute(&mut *tx).await?;
sqlx::query("INSERT INTO task_audit_log (task_id, from_status, to_status, worker_id, trace_id) ...")
    .execute(&mut *tx).await?;
tx.commit().await?;
```

Same connection, same transaction — atomicity guaranteed (NFR-C2). A committed state change without a corresponding audit row is impossible by construction.

#### G2 Interaction

When transactional enqueue (G2) is used with audit logging enabled, the task creation audit entry is inside the caller's transaction. Rollback erases both the task and its audit entry — no phantom rows.

### Cross-Feature Interaction Matrix

| Feature Pair | Interaction | Resolution |
|---|---|---|
| G1 + G2 | Idempotency key inside caller's txn | Key uniqueness check uses same txn — duplicate detection is txn-scoped |
| G2 + G5 | Audit entry inside caller's txn | Rollback erases both task and audit entry — no phantoms |
| G3 + G5 | UNLOGGED + audit log | Mutual exclusion enforced at startup (FR40). Cannot enable both |
| G4 + G5 | Trace ID in audit log | G5 schema includes `trace_id` column populated from G4. G4 must ship before or with G5 |
| G6 + G7 | Checkpoint before suspend | Hard dependency: `ctx.suspend()` calls `ctx.checkpoint()` internally before releasing slot |
| G7 + Sweeper | Suspended tasks | Sweeper skips Suspended in zombie recovery; suspend watchdog is separate clause |
| G8 + G1 | Region + idempotency | Independent: idempotency key is scoped per-queue, region is per-task. No interaction |
| G8 + G4 | Region label in traces | `region` added as span attribute when present |

### Implementation Order

Based on dependency analysis and risk assessment:

```
G1 (idempotency) → G2 (txn enqueue) → G4 (traces) → G5 (audit log) → G3 (UNLOGGED) → G6 (checkpoint) → G8 (geo pinning) → G7 (HITL)
```

**Rationale:**
- G1 and G2 are independent Tier 1 features with no schema dependencies
- G4 must precede G5 (audit log needs `trace_id` column)
- G3 follows G5 (mutual exclusion testing)
- G6 before G7 (hard blocking dependency)
- G8 is independent, placed after G6 for capacity reasons
- G7 last (highest complexity, most dependencies)

### Migration Strategy

All Growth migrations are additive `ALTER TABLE ADD COLUMN` operations — no data migration required for existing rows. New columns are nullable, defaulting to NULL. Existing tasks function identically before and after migration.

**Migration numbering:** Growth migrations continue the existing sequence (MVP migrations are `0001_*` through `000N_*`). Each Growth feature gets its own numbered migration file for independent rollback.

**Rollback:** Each migration has a corresponding `down.sql` dropping the added columns/tables. Rolling back G5 (audit log) also drops the immutability trigger. Rolling back G7 removes the `Suspended` status from any tasks in that state (migration fails if suspended tasks exist — operator must resolve first).