---
stepsCompleted: ['step-01-init', 'step-02-discovery', 'step-02b-vision', 'step-02c-executive-summary', 'step-03-success', 'step-04-journeys', 'step-05-domain', 'step-06-innovation', 'step-07-project-type', 'step-01b-continue', 'step-08-scoping', 'step-09-functional', 'step-10-nonfunctional', 'step-11-polish', 'step-12-complete', 'step-e-01-discovery', 'step-e-02-review', 'step-e-03-edit']
lastEdited: '2026-04-24'
editHistory:
  - date: '2026-04-24'
    changes: 'Growth phase expansion: 8 measurable outcomes, 8 tiered feature specs (G1-G8) with ACs/constraints/deps, 3 user journeys, Phase 2 scoping tables, 22 FRs (FR45-FR66), 8 NFRs (R7-R9, C1-C3, SC5-SC6)'
  - date: '2026-04-24'
    changes: 'Party mode review fixes: G3 moved from Tier 1→Tier 2; G1 conflict behavior + index predicate + retention mechanics specified; G2 isolation level pinned + G5 transaction semantics clarified; G5 regulatory frameworks + DB-level enforcement + trace_id column; G7 suspend watchdog + G6 hard dependency; G8 user problem statement + null-region fallback; NFRs gain p99 percentiles + reference environments; implementation order specified'
classification:
  projectType: 'Infrastructure Platform (embeddable Rust library + standalone service)'
  domain: 'Enterprise Infrastructure / Distributed Systems'
  targetMarket: 'Regulated-industry enterprises (fintech, healthcare) and Rust-native engineering teams'
  complexity: 'high'
  projectContext: 'greenfield'
  dualAudience: 'developers (library API ergonomics) and operators (deployment, observability, compliance)'
inputDocuments: ['docs/artifacts/planning/research/domain-distributed-task-queue-durable-execution-research-2026-04-02.md', 'PROPOSAL.md']
workflowType: 'prd'
---

# Product Requirements Document - iron-defer

**Author:** Fabio
**Date:** 2026-04-02

## Executive Summary

**iron-defer** is a durable background task execution engine for Rust-native systems. It guarantees at-least-once execution of critical tasks using PostgreSQL as its sole runtime dependency — no message brokers, no separate servers, no distributed coordination layer beyond the database the application already owns.

The engine targets engineering teams that need Temporal-tier reliability guarantees but cannot justify Temporal's operational overhead (7 services, 512 history shards, Cassandra at scale) or pricing ($25–50/million actions). It is equally suited to teams that have moved to Rust for systems reliability and find the existing Rust job queue ecosystem (Apalis at 1.0.0-rc.7, sqlxmq, rexecutor-sqlx) either immature or absent of durable execution semantics.

iron-defer ships in two modes: as an embeddable Rust library integrated directly into an existing application's Tokio runtime, and as a standalone binary deployable via Docker Compose or Kubernetes. Both modes share the same Postgres-backed engine. The dual-target architecture — `lib.rs` for embedding, `main.rs` for standalone — means teams can start embedded and extract to a dedicated service as scale demands, without changing their task logic.

**Primary users:** Rust software engineers implementing background task logic; platform/DevOps engineers operating the engine in production; engineering managers and architects evaluating operational footprint and compliance posture.

**Core problem solved:** Background tasks that must not be lost. A payment confirmation, a report generation, an audit event emission — these cannot be fire-and-forget. They require a guarantee: *the task will run, or the system will know why it didn't and try again.* Existing solutions in the Rust ecosystem do not provide this guarantee at production quality.

### What Makes This Special

**Single-dependency operational model.** PostgreSQL is already running in every deployment iron-defer targets. Adding iron-defer adds zero new infrastructure. The contrast with Temporal (7 services) or Faktory (separate Go server) is immediate and concrete.

**Memory-safe runtime as a trust signal.** Rust's ownership model eliminates buffer overflows, use-after-free, and data races at compile time. For regulated-industry buyers evaluating infrastructure under DORA, ISO 27001, and PCI DSS supply chain security requirements, this is a verifiable, documented property — not a marketing claim.

**Compliance-native design.** On-premises deployment satisfies GDPR Chapter V and HIPAA data residency requirements that block Temporal Cloud adoption in financial services and healthcare. The Postgres-backed audit trail satisfies PCI DSS Requirement 10, SOC 2 CC7.2, and DORA incident reporting obligations natively, without bolt-on logging infrastructure.

**Cost model transparency.** The cost of running iron-defer is the cost of a Postgres instance. No per-action pricing, no support tier gates, no surprise bills at scale.

**Reduced learning curve.** Temporal's determinism constraints require a month of onboarding. iron-defer's model — submit a task, a worker picks it up, the result is recorded — requires no new programming model.

## Project Classification

- **Project Type:** Infrastructure Platform — embeddable Rust library (`lib.rs`) + standalone service binary (`main.rs`)
- **Domain:** Enterprise Infrastructure / Distributed Systems
- **Target Market:** Rust-native engineering teams; regulated-industry enterprises (fintech, healthcare) requiring self-hosted durable execution
- **Complexity:** High — distributed concurrency, lease management, Postgres internals, compliance requirements
- **Project Context:** Greenfield
- **Capstone context:** This project is also a personal capstone — architecturally intentional, designed to demonstrate production-grade Rust systems engineering

## Success Criteria

### User Success

- **Crash-recovery visibility:** A developer witnesses a worker process die mid-execution and observes the task automatically claimed and completed by another worker — without manual intervention. This is the primary "this is exactly what I needed" moment.
- **Full task observability:** Every task's lifecycle state (Pending, Running, Completed, Failed, retrying) is visible through a dashboard and queryable via metrics/traces without requiring custom logging or SQL queries.
- **Time-to-first-task:** A developer with an existing Rust + Postgres project goes from `cargo add iron-defer` to a task executing in production in a single development session. No new infrastructure provisioned, no external service configured.

### Business Success

- **Open-source adoption:** iron-defer becomes the de facto Rust-native durable task queue — the answer teams find when they search "background jobs Rust production."
- **Industry recognition:** Used in production by at least one enterprise or notable OSS project within 12 months of first stable release.
- **Capstone excellence:** Demonstrates mastery of production-grade Rust systems engineering — distributed concurrency, Postgres internals, observability standards, and resilience patterns.

### Technical Success

- **At-least-once execution semantics:** Every task executes at least once. Under failure conditions (worker crash, network partition, process restart), the task is recovered and retried — never silently dropped. Duplicate execution is prevented under normal conditions by the atomic claiming engine; under lease-expiry recovery it is bounded and detectable. True exactly-once requires application-level idempotency keys (Growth phase).
- **OpenTelemetry integration (MVP):** Metrics and logs emitted via the OTel SDK with OTLP export. Any OTel-compatible backend (Prometheus, Jaeger, Grafana, Datadog, Honeycomb) receives data without custom configuration. Traces and Events are Growth phase additions; W3C trace-context propagation across the enqueue/dequeue boundary is a Growth deliverable.
- **Throughput baseline:** 10,000 jobs/second on a single commodity Postgres instance as the MVP target. Architecture must not impose ceilings — Postgres sharding (pg_partman), Aurora, Citus, and SKIP LOCKED tuning are supported scalability paths, not afterthoughts.
- **Resilience under chaos:** Integration tests using `testcontainers-rs` verify task recovery when worker processes are killed mid-execution.
- **Graceful shutdown:** Workers receiving SIGTERM complete in-flight tasks (or release leases cleanly) before terminating — no orphaned Running tasks on planned shutdowns.

### Measurable Outcomes

| Outcome | Target | How Measured |
|---|---|---|
| Task recovery after worker crash | < lease duration (configurable, e.g. 5 min) | Chaos integration tests |
| Throughput (MVP baseline) | ≥ 10,000 jobs/sec | Benchmark suite (single Postgres instance) |
| Time-to-first-task (new project) | < 30 minutes | Developer onboarding docs + demo |
| OTel signal coverage (MVP) | Metrics + Logs | Test suite against OTel Collector |
| Zero task loss under crash | 100% recovery in chaos tests | testcontainers-rs chaos suite |
| At-least-once guarantee | Zero duplicate completions under normal conditions in chaos tests | Integration test assertions |

### Growth-Phase Measurable Outcomes

| Outcome | Target | How Measured |
|---|---|---|
| Exactly-once submission | Zero duplicate tasks when submitter retries with same idempotency key | Integration tests: 10 concurrent retries with same key → 1 task created |
| Transactional enqueue atomicity | Zero orphaned tasks when enclosing transaction rolls back | Integration tests: enqueue inside rolled-back txn → 0 visible tasks |
| Audit log completeness | 100% of task state transitions captured in audit log table | Integration tests: N tasks through full lifecycle → N×(transitions) audit rows |
| OTel trace propagation | W3C traceparent header preserved across enqueue→claim→execute boundary | Integration tests: submit with traceparent → worker span has same trace ID |
| Checkpoint/resume correctness | Zero data loss on worker crash mid-workflow | Chaos tests: kill worker at each checkpoint → resume produces correct final state |
| HITL round-trip latency | Task resumes within 1s of signal delivery | Integration tests: suspend → signal → measure resume latency |
| Geographic pinning accuracy | 100% of pinned tasks execute on correctly-labeled workers | Integration tests: submit with region label → verify worker label matches |
| UNLOGGED throughput gain | ≥ 5× throughput improvement over WAL-logged mode | Benchmark suite comparison (same hardware, same task shape) |

## Product Scope

### MVP — Minimum Viable Product

*Aligned with the 4-week PROPOSAL.md roadmap:*

**Week 1 — Core Persistence & Inbound Interface**
- Dual-target Cargo project (`lib.rs` embedded engine + `main.rs` standalone binary)
- PostgreSQL schema: tasks table, state machine (`Pending → Running → Completed / Failed`), metadata (`retry_count`, `claimed_until`, `payload`)
- REST API (axum): task submission and queue inspection
- CLI (clap): operator task submission and queue state inspection
- `serde` / `serde_json` payload serialization strategy

**Week 2 — Distributed Execution & Concurrency**
- `SKIP LOCKED` claiming engine for distributed coordination
- Tokio async worker pool with configurable concurrency limits
- `Task` trait abstraction (HTTP webhooks, shell commands, embedded Rust functions)
- Error handling and retry logic (immediate + delayed)

**Week 3 — Resilience & Observability**
- Sweeper/Reaper: background process recovering zombie tasks (Running with expired lease)
- `tracing` crate structured logging with `task_id` correlation
- OpenTelemetry metrics (queue depth, execution latency, retry rate, failure rate)
- SIGTERM graceful shutdown with lease release

**Week 4 — Optimization, Hardening & Review**
- Docker image + Docker Compose and Kubernetes deployment manifests
- Postgres index optimization for the Fetcher loop
- Exponential backoff for retries
- Chaos integration tests (`testcontainers-rs`: kill workers mid-execution, verify recovery)

### Growth Features (Post-MVP)

Growth features are organized into three tiers by adoption impact. Tier 1 removes friction for production deployments (duplicate submission and dual-write bugs). Tier 2 extends observability, compliance, and performance (traces, audit log, UNLOGGED mode). Tier 3 delivers differentiated capabilities for advanced workloads (checkpoint/resume, HITL, geographic pinning).

#### Tier 1 — Production Adoption Enablers

**G1. Exactly-once submission with idempotency keys**

Client-supplied idempotency key on task submission prevents duplicate task creation when submitters retry after network errors or timeouts. The key is a unique constraint on the tasks table; a second `INSERT` with the same key returns the existing task instead of creating a new one.

- *Acceptance criteria:* Submitting the same idempotency key N times creates exactly 1 task. The response for duplicate submissions returns HTTP 200 (not 201) with the original task record. The 9 "losing" concurrent submitters each receive the same HTTP 200 response — no 500s, no ambiguous errors. Keys are scoped per-queue (same key in different queues creates separate tasks). Key uniqueness is enforced at the Postgres level (unique partial index). Expired or completed tasks release their keys after a configurable retention window (`idempotency_key_retention`, default 24h), enforced by the existing Sweeper tick.
- *Technical constraints:* Requires a new `idempotency_key VARCHAR` column and `idempotency_expires_at TIMESTAMPTZ` column on the tasks table. Unique partial index: `CREATE UNIQUE INDEX ON tasks (queue, idempotency_key) WHERE idempotency_key IS NOT NULL AND status NOT IN ('completed', 'failed', 'cancelled')`. The partial predicate scopes uniqueness to active tasks only — terminal tasks release their keys automatically. Key retention cleanup piggybacks on the existing Sweeper tick (no new background actor). On conflict, the engine performs `INSERT ... ON CONFLICT (queue, idempotency_key) WHERE ... DO NOTHING` followed by a `SELECT` to return the existing row. Key is optional — omitting it preserves current at-least-once behavior.
- *Dependencies:* Sweeper internals (retention cleanup added to existing sweep cycle).

**G2. Transactional enqueue (River pattern)**

Tasks inserted inside a caller-provided database transaction become visible to workers only when the transaction commits. If the transaction rolls back, the task vanishes — eliminating the dual-write bug class where a task is enqueued but the business event that triggered it fails to persist.

- *Acceptance criteria:* `engine.enqueue_in_tx(&mut tx, queue, task)` accepts a `&mut sqlx::Transaction` and inserts the task row inside that transaction. Tasks are invisible to workers until commit — guaranteed by Postgres MVCC at the default `READ COMMITTED` isolation level (uncommitted rows are invisible to other transactions' `SELECT ... FOR UPDATE SKIP LOCKED`). A rolled-back transaction produces zero visible tasks and zero worker activations during the rollback window. The REST API does not support transactional enqueue (embedded-library-only capability). When G5 (audit log) is enabled, the audit log entry for task creation is written inside the same caller transaction — a rollback erases both the task and its creation audit entry (correct: no phantom audit rows for non-existent tasks).
- *Technical constraints:* Requires a new engine method that accepts a `&mut sqlx::Transaction<'_, Postgres>`. No schema change required. The engine must not acquire additional locks or issue additional queries beyond the single `INSERT INTO tasks`. The engine must not hold its own transaction open during the caller's transaction. Isolation level is not pinned by iron-defer — the caller's transaction inherits the pool's default (`READ COMMITTED`); iron-defer documents that `SERIALIZABLE` is supported but not required.
- *Dependencies:* None for core feature. G5 (audit log) interaction documented in AC above.

#### Tier 2 — Observability, Compliance & Performance

**G4. Full OTel 4-pillar coverage (Traces + Events)**

Extend OTel integration from MVP's Metrics + Logs to full 4-pillar: Traces (distributed spans per task execution) and Events (structured lifecycle events as OTel Log Records). W3C `traceparent` header propagated across the enqueue→claim→execute boundary.

- *Acceptance criteria:* Each task execution produces a span with `task_id`, `queue`, `kind`, and `attempt` attributes. The span's trace ID matches the `traceparent` header supplied at enqueue time (if present). Task state transitions emit OTel Events with structured attributes. Traces are exported via OTLP alongside existing metrics. A task submitted with a `traceparent` header produces a child span visible in Jaeger/Tempo.
- *Technical constraints:* Requires `opentelemetry` tracing integration in the worker dispatch path. The `TaskContext` must carry the incoming trace context. Span creation must not block the worker's poll loop. OTLP trace export uses the same endpoint as metrics. Trace context is stored as a `trace_id VARCHAR` column on the tasks table so G5 (audit log) can include it. Test infrastructure requires either a test-mode in-memory span exporter or an OTLP collector testcontainer (Jaeger all-in-one) for integration tests.
- *Dependencies:* MVP OTel metrics infrastructure. The `TaskContext` struct (expanded in Epic 6) carries the trace context. Must ship before or concurrently with G5 so the audit log schema includes `trace_id`.

**G5. Append-only audit log table**

A dedicated `task_audit_log` table records every task state transition as an immutable, append-only row. Each row captures: `task_id`, `from_status`, `to_status`, `timestamp`, `worker_id`, `metadata`. The table is insert-only at the application level — no `UPDATE` or `DELETE` queries are emitted against it.

- *Acceptance criteria:* Every task state transition (`Pending→Running`, `Running→Completed`, `Running→Failed`, etc.) creates a new audit log row in the **same database transaction** as the state change — a committed state transition without a corresponding audit row is a system failure. The audit log table has no `UPDATE` or `DELETE` operations in the codebase, enforced by a Postgres `BEFORE UPDATE OR DELETE` trigger that raises an exception (database-level enforcement, not application-level only). An operator can query the complete lifecycle of any task via `SELECT * FROM task_audit_log WHERE task_id = $1 ORDER BY timestamp`. The log satisfies PCI DSS Req. 10 (timestamp, actor, action, outcome), SOC 2 CC7.2 (system-level audit trail), and DORA Art. 11 (incident reconstruction). Each audit row includes `trace_id` (from G4) when available, enabling trace-to-audit correlation. Mutual exclusion with UNLOGGED mode is enforced. When G2 (transactional enqueue) is used, the task creation audit entry is inside the caller's transaction — rollback erases both task and audit entry (no phantom audit rows).
- *Technical constraints:* Requires a new migration creating the `task_audit_log` table with columns: `id`, `task_id`, `from_status`, `to_status`, `timestamp`, `worker_id`, `trace_id`, `metadata`. Audit inserts use the **same connection and transaction** as the state transition (not a separate connection — separate connections break atomicity). A `BEFORE UPDATE OR DELETE` trigger on `task_audit_log` raises `'audit log is append-only'` to enforce immutability at the database level. Index on `(task_id, timestamp)` for per-task queries. Partition-ready schema (by timestamp) for retention management.
- *Dependencies:* G3 (UNLOGGED mutual exclusion logic). G4 (trace_id column — G4 must ship first or concurrently).

**G3. Configurable UNLOGGED table mode**

An operator running high-frequency, non-durable workloads (e.g., ephemeral task queues for batch processing, development/staging environments, load testing) can configure iron-defer to use Postgres `UNLOGGED` tables, eliminating WAL writes for significant throughput improvement.

- *Acceptance criteria:* A `database.unlogged_tables: true` configuration flag causes the migration to create `UNLOGGED` tasks tables. The engine rejects startup with a clear error if both `unlogged_tables` and `audit_log` are enabled simultaneously. Documentation explicitly states — at the configuration layer, not just in comments — that UNLOGGED tables are truncated on Postgres crash recovery, making them unsuitable for durable workloads. A benchmark comparison on production-configured Postgres (tuned `shared_buffers`, `max_wal_size`, `checkpoint_completion_target`) demonstrates ≥ 5× throughput improvement. The benchmark runs on dedicated hardware, not in CI.
- *Technical constraints:* Requires a conditional migration path (`CREATE UNLOGGED TABLE` vs `CREATE TABLE`). Existing deployments switching modes require a table recreate (data migration documented). Mutual exclusion with audit log is enforced at startup, not at schema level. Benchmark must use a production-like Postgres configuration — testcontainers default config does not exhibit representative WAL overhead.
- *Dependencies:* FR40 (mutual exclusion) is already implemented in MVP. Growth adds the actual UNLOGGED migration path.

#### Tier 3 — Differentiated Capabilities

**G6. Checkpoint/resume for multi-step workflows**

A task handler can persist intermediate checkpoint data during execution. If the worker crashes mid-workflow, the next worker resumes from the last checkpoint instead of restarting from scratch. Checkpoints are stored in a `jsonb` column on the tasks table.

- *Acceptance criteria:* `ctx.checkpoint(data: serde_json::Value)` persists checkpoint data to the task row atomically. On retry, `ctx.last_checkpoint()` returns the most recent checkpoint data (or `None` for first attempt). A task that checkpoints at step 3 of 5, then crashes, resumes at step 3 on the next attempt. Checkpoint data is cleared on task completion. Checkpoints are visible via `GET /tasks/{id}` response (new `lastCheckpoint` field).
- *Technical constraints:* Requires a new `checkpoint jsonb` column on the tasks table. The `checkpoint()` call issues an `UPDATE tasks SET checkpoint = $1 WHERE id = $2` — one DB round-trip per checkpoint. Checkpoint size is bounded by the existing 1 MiB payload limit. The `TaskContext` trait must expose checkpoint methods.
- *Dependencies:* None — additive schema change.

**G7. HITL (Human-in-the-Loop) suspend/resume**

A task handler can suspend execution, yielding its worker slot, and wait for an external signal to resume. The suspended task is not counted against concurrency limits. An external caller delivers the signal via `POST /tasks/{id}/signal` with an optional payload.

- *Acceptance criteria:* `ctx.suspend()` transitions the task to a new `Suspended` status, releases the worker slot (including the `SKIP LOCKED` advisory lock and the database connection), and returns control to the worker pool. `POST /tasks/{id}/signal` with a JSON body transitions the task back to `Pending` with the signal payload accessible via `ctx.signal_payload()` on the next execution. Suspended tasks do not count against `concurrency` limits. Suspended tasks do not trigger the Sweeper (no lease expiry while suspended). A **suspend watchdog** (piggybacking on the existing Sweeper tick) auto-fails tasks suspended longer than the configurable `suspend_timeout` (default: 24h). Two concurrent signals to the same suspended task: exactly one succeeds (returns 200), the other receives 409 `TASK_NOT_IN_EXPECTED_STATE`.
- *Technical constraints:* Requires a new `Suspended` variant in `TaskStatus` (the enum is already `#[non_exhaustive]`). Requires new columns: `signal_payload jsonb`, `suspended_at TIMESTAMPTZ`. Requires a new REST endpoint (`POST /tasks/{id}/signal`). The Sweeper gains a suspend watchdog clause: `WHERE status = 'suspended' AND suspended_at < NOW() - suspend_timeout`. Worker slot accounting must exclude suspended tasks. **Critical implementation constraint:** `ctx.suspend()` must release the database connection held by the worker's `SKIP LOCKED` claim before returning. The current worker loop holds the connection for the full execution duration — suspend breaks this assumption and requires the worker to checkpoint state (via G6) before releasing.
- *Dependencies:* **G6 (checkpoint/resume) is a hard blocking dependency.** A suspended task must persist its intermediate state via checkpoint before releasing its worker slot. Without G6, suspend yields the slot but loses all in-memory execution state, making resume useless. G6 must be fully implemented and tested before G7 development begins.

**G8. Geographic worker pinning**

Regulated-industry deployments (GDPR Chapter V, HIPAA data residency) require that tasks containing region-specific data execute only on workers within the designated geography. Tasks can be submitted with a region label. Workers register with a region label at startup. The claiming engine routes labeled tasks to matching workers only. Unlabeled tasks are claimed by any worker (backward-compatible).

- *Acceptance criteria:* `engine.enqueue_with_region(queue, task, "eu-west")` submits a region-pinned task. Workers configured with `worker.region: "eu-west"` claim only tasks labeled `eu-west` **or** tasks with no region label (NULL). A task labeled `eu-west` is never claimed by a worker labeled `us-east`. Workers with no region configured (`worker.region` absent or empty) claim **only** unlabeled tasks — they do not claim region-pinned tasks. Unlabeled tasks are claimed by any worker regardless of region. Region labels are visible in queue statistics and metrics (`region` label on OTel instruments).
- *Technical constraints:* Requires a new `region VARCHAR` column on the tasks table (nullable — NULL means unpinned). The `SKIP LOCKED` claiming query adds: `WHERE (region IS NULL OR region = $worker_region)` when the worker has a region, or `WHERE region IS NULL` when the worker has no region. Worker region is a configuration field, not dynamic. Region is an opaque string — no validation beyond non-empty.
- *Dependencies:* None — additive schema and query change. User need documented in Journey 4 (Architect/Compliance) and Domain Requirements (GDPR, HIPAA data residency).

### Vision (Future)

iron-defer becomes the **industry-standard open-source durable task execution engine for Rust** — the answer every Rust engineering team reaches for when they need background tasks that must not be lost.

- First-class AI agent orchestration substrate (checkpoint/resume, HITL, step-level retries)
- Multi-backend support: Aurora, Citus, CockroachDB as validated scalability paths
- SBOM publication, security policy, and compliance documentation for regulated-industry procurement
- Community ecosystem: plugins, worker SDKs, framework integrations (axum, actix)

## User Journeys

### Journey 1: The Rust Engineer — Happy Path
**Persona: Marco, Senior Backend Engineer at a B2B payments startup**

Marco's team is migrating from a Python monolith to a Rust microservices architecture. Payment webhooks are unreliable — when delivery fails, the event disappears. The Rust ecosystem has nothing production-ready. He finds iron-defer after a week of frustration with Apalis dropping tasks after worker restarts.

**Opening scene:** Marco adds `iron-defer` to `Cargo.toml`. He already runs Postgres. No new infrastructure to provision.

**Rising action:** He defines a `PaymentWebhookTask` implementing the `Task` trait — 15 lines of Rust. He submits a task via the embedded library inside his existing axum request handler. A worker picks it up within milliseconds. He checks the metrics endpoint: queue depth, latency histogram, retry count — all there in Prometheus.

**Climax:** Marco writes a chaos test. He submits 100 tasks, kills the worker process mid-execution. The Sweeper recovers all in-flight tasks. Every one of the 100 tasks completes exactly once. The test passes on the first run.

**Resolution:** Six months later, not a single payment webhook has been lost. The on-call rotation no longer has a "check the job queue" runbook step.

*Capabilities revealed: embedded library API, Task trait, Postgres schema, Sweeper/Reaper, OTel metrics, chaos recovery.*

---

### Journey 2: The Rust Engineer — Edge Case (Failure & Recovery)
**Persona: Marco (same), debugging a stuck task**

Three months after launch, Marco gets a Slack alert: a task has been in `Running` state for 40 minutes. The worker that claimed it is dead. The lease expired. The Sweeper hasn't recovered it — its interval was misconfigured to 60 minutes.

**Opening scene:** Marco queries the tasks table. Status is `Running`, `claimed_until` is in the past. He checks the `tracing` logs — every line tagged with `task_id`. Worker claimed the task, then went silent. A downstream HTTP call timed out and the worker panicked.

**Rising action:** Marco reconfigures the Sweeper interval to 5 minutes via CLI. He adds retry limits to the task definition: max 3 attempts, exponential backoff.

**Climax:** The Sweeper recovers the zombie task. The next worker picks it up; the downstream service has recovered; the task completes. The full trace — original attempt, failure, recovery, retry, completion — is visible as a single distributed trace in Jaeger.

**Resolution:** Marco adds a Prometheus alert: `iron_defer_zombie_tasks_total > 0` for 10 minutes. He now catches stuck tasks before they become customer incidents.

*Capabilities revealed: Sweeper configuration, structured logging with task_id, retry/backoff configuration, CLI inspection, distributed tracing across retries.*

---

### Journey 3: The Platform Engineer — Operator Journey
**Persona: Dana, Platform Engineering Lead at a healthcare software company**

Dana doesn't write Rust. Her team owns the infrastructure. The dev team wants iron-defer in standalone mode — a dedicated binary in Kubernetes.

**Opening scene:** Dana pulls the iron-defer Docker image, applies the provided Kubernetes manifests — a Deployment and a ConfigMap pointing at the existing Postgres connection string. Workers start. No sidecars, no extra dependencies.

**Rising action:** She configures `terminationGracePeriodSeconds: 60`. When Kubernetes evicts a worker Pod, iron-defer receives SIGTERM, drains in-flight tasks, releases leases cleanly, and exits. No zombie tasks. She wires the `/metrics` endpoint to the cluster's Prometheus. Within 5 minutes she has a Grafana dashboard: queue depth, worker pool utilization, p99 execution latency.

**Climax:** A Postgres maintenance window causes a 3-minute outage. Queue depth climbs (tasks submitted but not claimed). When Postgres recovers, workers resume. No tasks were lost. The backlog drains in under 2 minutes.

**Resolution:** Dana adds iron-defer to the team's standard platform template. New services get durable task execution by default. The compliance team signs off: task history is queryable in Postgres, satisfying the HIPAA audit trail requirement.

*Capabilities revealed: Docker image, Kubernetes manifests, SIGTERM graceful shutdown, Prometheus/Grafana integration, Postgres connection resilience.*

---

### Journey 4: The Architect / Compliance Evaluator
**Persona: Jordan, VP of Engineering at a fintech company evaluating infrastructure**

Jordan's team needs Temporal-tier reliability for transaction processing. Temporal Cloud is off the table — legal flagged data residency. Self-hosted Temporal requires 7 services. A junior engineer sends Jordan a link to iron-defer.

**Opening scene:** Jordan reads the README. Single Postgres dependency. Embeddable or standalone. Rust. Memory-safe. He forwards it to the security architect: "Can this satisfy our DORA audit trail requirements?"

**Rising action:** Jordan runs through the compliance checklist. Structured logs with task IDs? Yes. Append-only history? Yes. OTel metrics for incident reporting? Yes — OTLP export to their existing backend. Data never leaves their VPC? Yes — single Postgres instance, no cloud calls.

**Climax:** Jordan's architecture review covers all failure modes: worker crash → Sweeper recovers; Postgres outage → tasks queue, workers retry connection; network partition → lease expires, task re-claimed. Every failure mode has a documented recovery path.

**Resolution:** Jordan approves iron-defer for a pilot. Three months later it's in two production services. The DORA audit passes — every transaction task has a complete, queryable audit trail in Postgres.

*Capabilities revealed: compliance documentation, structured audit trail, OTLP export, self-hosted deployment model, failure mode documentation.*

---

### Journey 5: The API Consumer — Non-Rust Service Integration
**Persona: Priya, Python Data Engineer submitting batch processing jobs**

Priya's team runs Python ML pipelines that need to submit jobs to the shared iron-defer service without writing Rust.

**Opening scene:** Priya hits `POST /tasks` with a JSON payload — task type, parameters, requested execution time. She gets a `task_id` back.

**Rising action:** She polls `GET /tasks/{task_id}` to check status. She watches the task transition: `Pending → Running → Completed`. She writes a small Python helper encapsulating the API calls.

**Climax:** During a high-volume batch run, one task fails with a downstream timeout. Priya sees the task move to `Failed`, `retry_count: 1`, then back to `Pending`. It completes on the second attempt. She never had to handle the retry in Python.

**Resolution:** Priya's team submits all batch ML jobs through iron-defer. They get automatic retry, execution history, and OTel traces for cross-service debugging — through a simple REST API, no Rust required.

*Capabilities revealed: REST API (task submission, status polling, result retrieval), retry visibility, cross-language compatibility.*

---

### Journey 6: The Telecom BSS Orchestrator — Order Fulfilment
**Persona: iron-defer as the task scheduler inside a future Telecom BSS platform**

A broadband service order triggers a multi-step fulfilment chain: credit check → inventory reservation → network provisioning → billing account creation → welcome notification. Each step calls a different system. Any one can fail. The sequence must complete exactly once or the customer gets charged twice or provisioned never.

**BSS v1 — MVP scope:** The BSS order management service submits independent, idempotent tasks for each fulfilment step. Inter-step state is managed by the BSS service in its own database — iron-defer is responsible for executing each task durably, not for passing state between steps. Chained enqueue is the pattern: on task completion, the BSS service callback enqueues the next task.

**Climax:** During a high-volume promotion, the network provisioning API becomes intermittently unavailable. Hundreds of orders are in-flight. Every `NetworkProvisioningTask` enters retry cycles with exponential backoff. Operations monitors queue depth and retry rate on Grafana. When the API recovers, the backlog drains automatically. No order is lost. SLA compliance is maintained.

**Resolution:** The complete audit trail in Postgres — every task state transition, every retry, every timestamp — satisfies telco regulatory reporting. When a customer disputes their activation date, the answer is one SQL query away.

**BSS v2 — Growth scope:** With checkpoint/resume and multi-step workflow primitives (Growth phase), iron-defer manages inter-step state natively. The BSS service delegates workflow state entirely to iron-defer, enabling rollback semantics and HITL pause points for high-value orders.

*Capabilities revealed: chained task submission, exponential backoff, Sweeper under sustained downstream failure, SLA-grade OTel metrics, audit trail for regulatory reporting, future path to checkpoint/resume.*

---

### Journey 7: The Migrating Engineer — From Existing Solution
**Persona: Sofia, Staff Engineer migrating a hand-rolled Tokio + SQLx queue**

Sofia's team built their own background job system 18 months ago. It works for simple cases but has a graveyard of lost tasks — recovery logic was buggy. She's evaluated Apalis (still RC), wants something stable and production-ready.

**Opening scene:** Sofia's concern isn't learning iron-defer — it's migrating without dropping a single live task. Her existing queue has ~200 in-flight tasks at any moment.

**Rising action:** She runs iron-defer's Postgres schema alongside her existing schema during transition. New task types go to iron-defer; the old system drains existing in-flight work. She adapts her existing task logic to the `Task` trait — the interface is simple enough to map her handlers directly.

**Climax:** She deploys iron-defer workers alongside the old workers. After 48 hours, the old queue is empty. She deletes the old schema and the old worker binary. No task was lost during the transition.

**Resolution:** Sofia's team immediately notices the observability difference — every task now has a trace, a retry count, a lease timestamp. The "ghost task" incidents stop.

*Capabilities revealed: schema migration path documentation, `Task` trait adaptability, parallel-deployment support, no-downtime rollout pattern.*

---

### Journey 8: The OSS Contributor *(Vision phase)*
**Persona: Kwame, open-source Rust developer extending iron-defer**

Kwame's team uses iron-defer in production and wants to add CockroachDB as a storage backend. He finds the storage trait abstraction well-documented, the plugin API clean, and the contributor guide complete.

*This journey belongs to the Vision phase. It informs the need for: documented plugin API, storage trait abstraction, and contributor onboarding guide. Out of scope for MVP.*

---

### Journey 9: The Rust Engineer — Idempotent Submission *(Growth — Tier 1)*
**Persona: Marco (same), hardening payment webhook submission**

Marco's payment service submits webhooks inside HTTP request handlers. Under load, the upstream gateway occasionally retries requests, causing duplicate task submissions. Marco needs exactly-once submission without changing his handler logic.

**Opening scene:** Marco adds an `idempotency_key` to his `enqueue()` call — the payment event's unique ID. He deploys.

**Rising action:** During a traffic spike, the gateway retries 50 requests. iron-defer returns the existing task for each duplicate key. Zero duplicate tasks are created. Marco's monitoring confirms: task count matches event count exactly.

**Resolution:** Marco removes his application-level deduplication table. iron-defer handles it at the engine level. One fewer moving part.

*Capabilities revealed: idempotency key submission, duplicate detection, HTTP 200 vs 201 distinction, key retention window.*

---

### Journey 10: The Platform Engineer — Audit & Compliance *(Growth — Tier 2)*
**Persona: Dana (same), preparing for PCI DSS audit**

Dana's compliance team needs tamper-evident evidence that every task state transition is recorded. The MVP's tasks table shows current state but not transition history.

**Opening scene:** Dana enables the `audit_log` configuration flag. iron-defer creates the `task_audit_log` table and starts recording every transition.

**Rising action:** During the PCI audit, the auditor asks: "Show me the complete lifecycle of transaction task X." Dana runs a single SQL query against `task_audit_log`. Every transition — Pending, Running, Failed (retry), Pending, Running, Completed — is there with timestamps and worker IDs.

**Climax:** The auditor also requests trace evidence. Dana opens Jaeger and shows the distributed trace spanning the entire task lifecycle — enqueue through final completion — with W3C trace context linking all attempts.

**Resolution:** PCI DSS Req. 10 audit passes. Dana's team adds the audit log query to their compliance automation.

*Capabilities revealed: append-only audit log, per-task transition history, OTel trace propagation, compliance evidence queries.*

---

### Journey 11: The AI Platform Engineer — Checkpoint/Resume *(Growth — Tier 3)*
**Persona: Yuki, ML Engineer running multi-step inference pipelines**

Yuki's team runs 5-step AI inference pipelines: data fetch → preprocess → model inference → post-process → delivery. Each step takes 2-10 minutes. Worker crashes at step 4 mean restarting from step 1 — wasting 15 minutes of GPU time.

**Opening scene:** Yuki adds `ctx.checkpoint()` calls after each pipeline step. Each checkpoint persists the intermediate result.

**Rising action:** A worker crashes during step 4. The sweeper recovers the task. The next worker calls `ctx.last_checkpoint()` and resumes from step 3's output. Steps 1-3 are not re-executed.

**Climax:** During a large batch run, Yuki adds HITL: a human reviewer must approve step 3's output before step 4 begins. The task suspends via `ctx.suspend()`, yielding its worker slot. The reviewer approves via `POST /tasks/{id}/signal`. The task resumes at step 4.

**Resolution:** Pipeline failure cost drops from 15 minutes to the duration of one step. HITL approval integrates naturally into the pipeline without polling or external coordination.

*Capabilities revealed: checkpoint persistence, crash-resume from last checkpoint, HITL suspend/signal/resume, worker slot release during suspension.*

---

### Journey Requirements Summary

| Journey | Key Capabilities Required | Phase |
|---|---|---|
| Rust Engineer — Happy Path | Library API, `Task` trait, embedded mode, OTel metrics, Sweeper, chaos recovery | MVP |
| Rust Engineer — Edge Case | Structured logging with task_id, retry/backoff config, CLI inspection, distributed traces | MVP |
| Platform Engineer | Docker image, Kubernetes manifests, SIGTERM shutdown, Prometheus integration, Postgres resilience | MVP |
| Architect / Evaluator | Audit trail, OTLP export, self-hosted model, failure mode docs, compliance posture | MVP |
| API Consumer | REST API (CRUD tasks), status polling, result payload retrieval, retry transparency | MVP |
| Telecom BSS Orchestrator v1 | Chained task submission, exponential backoff, Sweeper under sustained failure, SLA-grade OTel, audit trail | MVP |
| Telecom BSS Orchestrator v2 | Checkpoint/resume, native multi-step workflows, inter-step state management | Growth |
| Migrating Engineer | Schema migration docs, `Task` trait adaptability, parallel-deployment, no-downtime rollout | MVP |
| Rust Engineer — Idempotent Submission | Idempotency keys, duplicate detection, exactly-once submission | Growth |
| Platform Engineer — Audit & Compliance | Append-only audit log, OTel traces, W3C trace propagation, compliance queries | Growth |
| AI Platform Engineer — Checkpoint/Resume | Checkpoint persistence, crash-resume, HITL suspend/signal/resume | Growth |
| OSS Contributor | Plugin API, storage trait abstraction, contributor guide | Vision |

**Known gaps — explicitly out of MVP scope:**

- **Task authorization / access control:** Who can submit, cancel, or inspect tasks? Multi-tenant isolation for shared deployments. MVP assumes single-tenant, network-isolated deployment with no authN/authZ on the REST API.
- **Zero-downtime engine upgrade:** Rolling schema migrations and backward-compatible worker deployments during iron-defer version upgrades. MVP documents a drain-and-restart upgrade procedure.

## Domain Requirements

### Compliance Requirements

iron-defer operates in regulated-industry contexts where background task execution directly intersects with audit, data protection, and incident reporting obligations. The following frameworks apply:

| Framework | Relevant Obligations | iron-defer Coverage |
|---|---|---|
| **PCI DSS v4.0.1 Req. 10** | Audit trail for all system component access | Postgres task history table; append-only audit log (Growth) |
| **GDPR Art. 5 / Chapter V** | Data minimisation; no cross-border transfer without adequacy | On-premises deployment; payload schema under application control |
| **HIPAA Security Rule** | Audit controls; transmission integrity; access control | Postgres-backed audit trail; TLS for Postgres connection; structured logs |
| **DORA (EU 2022/2554)** | ICT incident reporting; operational resilience testing | OTel metrics for incident reconstruction; chaos testing evidence |
| **NIS2 Directive** | Supply-chain risk; dependency inventory | Minimal dependency surface (Postgres only); SBOM publication (Vision) |
| **SOC 2 CC7.2** | Detection of and response to security events | Structured task logs with correlation IDs; OTel alerting integration |
| **ISO 27001:2022** | Asset management; change control; logging | Queryable task history; Postgres as single state store |

**Testable acceptance criteria (compliance):**

- An OTel integration test spec must demonstrate that every task lifecycle event (Pending, Running, Completed, Failed, retry) emits a compliant OTel trace span and structured log record, both tagged with `task_id` and `queue_name`.
- The integration test suite must include a Postgres-query assertion that confirms the tasks table acts as a queryable audit trail: all state transitions are recorded with timestamps and actor identity (worker ID).
- Compliance documentation must reference the specific test file(s) that validate each framework obligation, so auditors can verify by running the test suite.

---

### Privacy Requirements

**Data minimisation:** iron-defer stores only the data necessary for task execution — task type, queue name, payload, state, timestamps, retry count, and lease expiry. The payload schema is entirely under the application's control; iron-defer imposes no structure.

**Payload redaction — design decision:**

Task payloads may contain PII (customer IDs, email addresses, order references). There is an explicit tension between:
- **Operational debugging value:** Full payload in error context enables fast root-cause analysis when tasks fail.
- **PII safety:** Payloads stored in Postgres and surfaced in logs/traces may violate GDPR data minimisation or HIPAA minimum-necessary principles.

**Resolution for MVP:** iron-defer stores payloads as opaque `jsonb` blobs. The engine does not log payload contents in structured log output or OTel traces by default. A configurable `log_payload: false` default ensures PII-safe behaviour out of the box. Applications that need payload-in-logs must opt in explicitly. Growth phase adds a payload redaction hook for sanitising payloads before error context capture.

**Data residency:** No task data leaves the Postgres instance. iron-defer makes no outbound calls except to the configured OTel Collector endpoint (operator-controlled). This satisfies GDPR Chapter V and HIPAA data residency requirements.

---

### Technical Constraints

**1. PostgreSQL as sole runtime dependency**

iron-defer requires PostgreSQL 14+ (for `SKIP LOCKED` and `pg_notify` availability). No other runtime services are required. The application's existing Postgres instance is sufficient for MVP; a dedicated instance is recommended for production workloads above 5,000 jobs/second.

**2. Bounded Postgres connection pool (embedded and standalone modes)**

Both the embedded library mode and the standalone binary mode must enforce a bounded connection pool. Unbounded pool growth causes Postgres `max_connections` exhaustion and cascades into application-level failures.

- **Embedded mode:** The caller must supply `pool_size: usize` at `IronDefer::new()`. The engine must reject construction if `pool_size` exceeds a configurable safety ceiling (default: 20 connections). The ceiling must be documented and configurable.
- **Standalone mode:** The Docker/Kubernetes deployment manifests must include `IRON_DEFER_POOL_SIZE` with a default of 10 and documented guidance on sizing relative to Postgres `max_connections`.
- Connection pool metrics (`pool_available`, `pool_in_use`, `pool_wait_queue_depth`) must be emitted as OTel gauges.

**3. UNLOGGED table mode and audit logging are mutually exclusive**

The Growth phase introduces configurable `UNLOGGED` table mode (~30x WAL reduction for high-throughput non-durable workloads). However, `UNLOGGED` tables are truncated on Postgres crash recovery, making them incompatible with audit trail requirements.

- **Startup validation:** If `unlogged_tables: true` and `audit_log: true` are both configured, iron-defer must refuse to start with an explicit error: `"UNLOGGED table mode and audit_log are mutually exclusive — UNLOGGED tables do not survive Postgres crash recovery and cannot satisfy audit trail requirements."`
- This constraint applies to both embedded and standalone modes.
- The configuration documentation must explicitly state this mutual exclusion with rationale.

**4. Tokio as sole async runtime**

iron-defer targets Tokio exclusively. `async-std` support is not planned (async-std is discontinued as of March 2025). The embedded library requires the caller to provide a Tokio runtime context. The standalone binary includes its own `#[tokio::main]` entry point.

**5. SQLx for Postgres interaction**

All database interaction uses `sqlx` with compile-time query verification. No runtime SQL generation or ORM abstraction. Migrations are embedded using `sqlx::migrate!()`.

---

### Integration Requirements

**OpenTelemetry (OTel) Integration**

iron-defer emits all observability signals via the OpenTelemetry SDK. The following signals are required for MVP:

| Signal | Content | Export |
|---|---|---|
| **Metrics** | Queue depth, execution latency (p50/p95/p99), retry rate, failure rate, worker pool utilisation, connection pool gauges | OTLP (gRPC or HTTP) |
| **Logs** | Structured log records per task lifecycle event; tagged with `task_id`, `queue_name`, `worker_id`, `attempt_number` | OTLP Logs or stdout JSON |
| **Traces** | Span per task execution; W3C trace-context propagated across enqueue/dequeue boundary (Growth) | OTLP |
| **Events** | Task state change events as OTel Events (Growth) | OTLP |

**OTel retention responsibility:** iron-defer emits signals to the configured OTel Collector endpoint. Signal retention, aggregation, alerting, and long-term storage are the responsibility of the operator's observability backend (Prometheus, Grafana, Datadog, Honeycomb, etc.). iron-defer does not buffer signals internally beyond the OTel SDK's export queue. This must be explicitly stated in the operations documentation to avoid operator misconfigurations that result in signal loss.

**Postgres Connectivity**

iron-defer accepts a standard `DATABASE_URL` (libpq-compatible connection string) or a pre-constructed `sqlx::PgPool`. TLS-encrypted connections are supported and recommended for production. The engine handles connection loss gracefully: workers back off and retry reconnection; the Sweeper resumes automatically when connectivity is restored.

**REST API (Standalone mode)**

The standalone binary exposes an HTTP API (axum) for task submission and status inspection. The API surface is documented via OpenAPI. No authentication is provided in MVP — deployments are expected to be network-isolated. Authentication (bearer token, mTLS) is a Growth phase addition.

**CLI**

The standalone binary exposes a `clap`-based CLI for operator interaction: task submission, queue inspection, worker status, configuration validation.

---

### Risk Mitigations

**Risk 1: Postgres outage causes task loss**

*Mitigation:* Tasks submitted during a Postgres outage fail at the submission call — the application retains responsibility for retry. Tasks already in `Pending` state persist through the outage. Workers reconnect automatically on recovery. The Sweeper resumes lease recovery after reconnection.

*Documentation requirement:* The operations guide must include a Postgres outage runbook: expected behaviour, queue depth monitoring, backlog drain estimation after recovery.

**Risk 2: Duplicate execution under split-brain / network partition**

*Mitigation:* SKIP LOCKED is atomic — only one worker claims a given task row. Lease expiry (`claimed_until`) is the recovery gate. The Sweeper only reclaims tasks whose lease has expired. Under normal conditions, duplicate execution is prevented by the atomic claim. Under sustained network partition (worker alive but disconnected from Postgres), the lease expires and another worker claims the task — this is at-least-once semantics at the boundary. Exactly-once requires application-level idempotency keys (Growth phase).

*Documentation requirement:* The developer guide must include an explicit section on idempotency design — how to write `Task` implementations that are safe to execute more than once.

**Risk 3: iron-defer depends on the Postgres DR posture of the deploying organisation**

iron-defer's durability guarantee is bounded by the durability of its Postgres instance. If the Postgres instance loses committed data (e.g., async replication lag during a failover, undetected corruption, or misconfigured backup policy), tasks in `Pending` or `Running` state may be lost.

*Mitigation:* iron-defer does not manage Postgres DR. The operations documentation must include an explicit DR posture statement:

> "iron-defer's task durability is bounded by your Postgres instance's durability. For production deployments, use synchronous replication (e.g., `synchronous_commit = on`), point-in-time recovery (PITR), and tested restore procedures. iron-defer has no independent DR mechanism."

*Deployment checklist:* The Kubernetes and Docker Compose deployment guides must include a DR readiness checklist as a prerequisite step.

**Risk 4: Worker memory leak under high-volume sustained load**

*Mitigation:* Tokio task handles are bounded by the configured worker pool size. The connection pool is bounded (see Technical Constraints §2). Metrics for pool utilisation and Tokio task count are emitted to allow operators to detect unbounded growth. Load testing at target throughput (10,000 jobs/sec) is part of the MVP benchmark suite.

**Risk 5: Chaos test coverage gaps**

*Mitigation:* The chaos integration test suite (`testcontainers-rs`) must include, at minimum:

1. **Worker kill mid-execution:** Kill the worker process while a task is `Running`; verify the Sweeper recovers the task and it completes exactly once.
2. **Postgres restart:** Stop and restart the Postgres container while tasks are in-flight; verify workers reconnect, the Sweeper resumes, and all tasks eventually complete. This scenario validates the connection pool reconnect logic and the Sweeper's restart recovery path — it is a mandatory chaos scenario, not optional.
3. **Sustained downstream failure:** Simulate a failing downstream HTTP endpoint for tasks that call external services; verify exponential backoff and eventual success when the endpoint recovers.
4. **Multiple concurrent workers:** Run N workers claiming from the same queue; verify no task is executed more than once (SKIP LOCKED correctness under concurrency).

*CI requirement:* Chaos tests must run in CI on every PR. Flaky chaos tests must be treated as blocking failures.

---

### Domain Requirements Summary

| Area | Key Constraint | Phase |
|---|---|---|
| Compliance | PCI DSS Req. 10, GDPR, HIPAA, DORA, SOC 2 audit trail | MVP (basic) / Growth (audit log) |
| Compliance testing | OTel integration test spec as machine-verifiable audit evidence | MVP |
| Privacy | Payload not logged by default; `log_payload: false` default | MVP |
| Privacy | Payload redaction hook for error context sanitisation | Growth |
| Technical | Bounded connection pool (embedded + standalone); pool metrics emitted | MVP |
| Technical | UNLOGGED + audit_log mutual exclusion enforced at startup | Growth |
| Technical | Tokio-only async runtime; SQLx compile-time queries | MVP |
| Integration | OTel OTLP export (metrics + logs MVP; traces + events Growth) | MVP / Growth |
| Integration | OTel retention is operator's responsibility — documented explicitly | MVP |
| Integration | REST API (axum) + CLI (clap); no authN in MVP | MVP |
| Risk | Postgres DR posture statement in operations docs | MVP |
| Risk | Chaos suite includes Postgres restart scenario | MVP |
| Risk | Idempotency design guide for exactly-once task implementations | MVP |

## Innovation & Novel Patterns

### Detected Innovation Areas

**1. First Production-Stable Durable Execution Engine in Rust**

The Rust ecosystem has background job libraries (Apalis at 1.0.0-rc.7, sqlxmq, rexecutor-sqlx) but none offer durable execution semantics at production quality. iron-defer is not a Rust port of an existing tool — it is purpose-built around Rust's ownership model, Tokio's async runtime, and PostgreSQL's transactional guarantees. The gap is real and confirmed by domain research.

*Innovation: occupying an empty category, not a crowded one.*

**2. Memory Safety as a Compliance Asset**

Temporal, Faktory, and River are built in Go. Rust's ownership model eliminates buffer overflows, use-after-free, and data races *at compile time*. For regulated-industry buyers evaluating infrastructure under DORA Article 8 (ICT risk management), NIS2 supply-chain security, and ISO 27001:2022 A.8.28 (secure coding), this is a verifiable, documented property that can be cited in compliance submissions — not a marketing claim.

*Innovation: first infrastructure library to position Rust's safety properties explicitly as compliance evidence.*

**3. Dual-Target Architecture — Embed-Then-Extract Migration Path**

iron-defer ships as a single codebase that compiles to both an embedded library (`lib.rs`) and a standalone service binary (`main.rs`). Teams start embedded (zero new infrastructure) and extract to a dedicated service as scale demands — without rewriting a single line of task logic. No existing durable execution solution offers this migration path.

*Innovation: embed-and-defer-the-infrastructure-decision pattern, new in this domain.*

**4. Postgres as the Entire Distributed Coordination Layer**

SKIP LOCKED turns Postgres row locking into a distributed, non-blocking worker election mechanism. No ZooKeeper, no etcd, no Redis, no Kafka. The coordination protocol is a 3-line SQL statement backed by decades of ACID guarantees. This is new in the Rust ecosystem and new in combination with a compliance-first positioning.

*Innovation: taking the "Postgres as infrastructure" insight mainstream in the Rust enterprise ecosystem.*

**5. Compliance-Native Design from Day One**

Most infrastructure tools bolt on compliance features after adoption. iron-defer treats compliance as a design constraint from the first commit: append-only audit log in the schema, OTel 4-pillar from the roadmap, GDPR data residency as a first-class deployment property.

*Innovation: treating regulated-industry requirements as product requirements, not afterthoughts.*

---

### Market Context & Competitive Landscape

iron-defer's innovation is at the intersection of three gaps: *Rust + production-stable + compliance-native*. No single competitor occupies all three positions:

- **Temporal**: USD 5B valuation, 7 services to self-host, $25–50/million actions, Go+Java SDK-primary, complex determinism model.
- **River (Go)**: Closest design analog — Postgres-native, SKIP LOCKED, ~10K jobs/sec baseline. Go, not Rust; no compliance positioning.
- **Apalis (Rust)**: Closest language match — 1.0.0-rc.7 as of March 2026, no durable execution semantics, no compliance positioning.
- **Faktory**: Standalone Go server, separate infrastructure, not embeddable.

---

### Validation Approach

| Innovation Claim | How to Validate | When |
|---|---|---|
| First production-stable Rust durable execution library | Publish 1.0.0; track crates.io downloads, GitHub stars, production reports within 6 months | Post-MVP |
| Memory safety as compliance evidence | Publish a compliance brief citing Rust's documented safety properties with references to DORA/ISO 27001 language | MVP docs |
| Embed-then-extract migration path | Documented, tested migration guide with reference architecture | MVP |
| Postgres coordination correctness | SKIP LOCKED correctness under N-worker concurrency chaos tests (pass/fail in CI) | MVP |
| Compliance-native audit trail | Integration test suite as machine-verifiable compliance evidence (OTel test spec) | MVP |

---

### Risk Mitigation

| Innovation Risk | Likelihood | Mitigation |
|---|---|---|
| Postgres scaling ceiling limits adoption | Medium | Document sharding paths (pg_partman, Aurora, Citus) as supported growth trajectories from Day 1 |
| Compliance claims oversell actual capabilities | Medium | Compliance documentation references specific test files as evidence; no unverified claims |
| Dual-target binary size or startup-time tradeoff | Low | Benchmark embedded vs. standalone mode; document the tradeoff explicitly |
| "Not invented here" resistance in Go/Java shops | High | Make Rust embedding story frictionless; REST API makes iron-defer accessible without Rust |

## Platform Requirements

### Project-Type Overview

iron-defer is a **Rust-native infrastructure library** with a standalone service binary as a secondary deployment target. The primary development experience is embedding the library into an existing Rust application. The REST API enables polyglot access to the standalone binary but is not the primary design surface — it is a deployment convenience, not a first-class SDK target.

This is a **Rust capstone project first**. The library API, Rust ergonomics, and crates.io distribution are the principal product surfaces. Non-Rust SDK development is explicitly deferred beyond the capstone.

---

### Language Matrix

| Surface | Languages Supported | Distribution | Phase |
|---|---|---|---|
| Embedded library | Rust only | crates.io (`cargo add iron-defer`) | MVP |
| Standalone REST API | Any (HTTP/JSON) | Docker Hub, GitHub Container Registry | MVP |
| CLI | N/A (operator tool) | Included in standalone binary | MVP |
| Non-Rust client SDKs | Not planned | — | Post-capstone Vision |

**Rust version policy:** iron-defer targets the current stable Rust toolchain. MSRV (Minimum Supported Rust Version) will be declared in `Cargo.toml` and enforced in CI. MSRV bumps require a minor version increment.

---

### Installation Methods

**Embedded library:**
```toml
# Cargo.toml
[dependencies]
iron-defer = "1.0"
```

All async primitives require a Tokio runtime. The caller provides the runtime context; iron-defer does not spawn its own.

**Standalone binary:**
```bash
# Docker
docker pull ghcr.io/feamcor/iron-defer:latest

# Docker Compose (provided manifest)
docker compose -f iron-defer/docker-compose.yml up

# Kubernetes (provided manifests)
kubectl apply -f iron-defer/k8s/
```

**Build from source:**
```bash
cargo build --release --bin iron-defer
```

**Feature flags (planned):**

| Flag | Default | Purpose |
|---|---|---|
| `metrics` | on | OTel metrics export |
| `tracing` | on | OTel trace export (Growth) |
| `audit-log` | off | Append-only audit log table (Growth; mutually exclusive with `unlogged`) |
| `unlogged` | off | UNLOGGED table mode (Growth; mutually exclusive with `audit-log`) |

---

### API Surface

The `Task` trait is the primary API contract. Everything else in the embedded library is configuration and runtime plumbing.

**Core trait:**
```rust
#[async_trait]
pub trait Task: Send + Sync + 'static {
    /// Unique string identifier for this task type (used for routing and logging)
    fn task_type() -> &'static str where Self: Sized;

    /// Execute the task. Return Ok(()) on success; Err on failure (triggers retry).
    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError>;
}
```

**Task submission:**
```rust
IronDefer::enqueue(queue, task, options).await?;
```

**Engine construction (embedded):**
```rust
let engine = IronDefer::builder()
    .pool(pg_pool)
    .pool_size(10)
    .worker_concurrency(20)
    .sweeper_interval(Duration::from_secs(300))
    .build()
    .await?;

engine.register::<PaymentWebhookTask>();
engine.start().await?;
```

**REST API surface (standalone):**

| Method | Path | Description |
|---|---|---|
| `POST` | `/tasks` | Submit a task |
| `GET` | `/tasks/{id}` | Get task status and result |
| `GET` | `/tasks` | List tasks (with queue/status filters) |
| `DELETE` | `/tasks/{id}` | Cancel a pending task |
| `GET` | `/queues` | List queues with depth and worker stats |
| `GET` | `/health` | Liveness probe |
| `GET` | `/ready` | Readiness probe (Postgres connectivity check) |
| `GET` | `/metrics` | Prometheus text exposition (scrape endpoint) |

**CLI surface (standalone):**

```
iron-defer submit --queue <queue> --type <task_type> --payload <json>
iron-defer inspect --queue <queue> [--status pending|running|failed]
iron-defer workers
iron-defer config validate
```

---

### Code Examples

All examples ship in the `examples/` directory of the repository. The following must be present at 1.0.0:

| Example | What it demonstrates |
|---|---|
| `examples/basic_task.rs` | Minimal embedded: define a task, submit it, verify completion |
| `examples/axum_integration.rs` | Embed iron-defer in an axum web server; enqueue tasks from HTTP request handlers |
| `examples/retry_and_backoff.rs` | Configure retry limits and exponential backoff; observe failure and recovery |
| `examples/sweeper_recovery.rs` | Simulate a worker crash; observe Sweeper recovering the zombie task |
| `examples/otel_integration.rs` | Wire OTel SDK to an OTLP Collector; verify metrics and logs are emitted |
| `examples/multi_queue.rs` | Multiple named queues with different concurrency configurations |
| `examples/docker-compose/` | Complete standalone deployment: iron-defer + Postgres + OTel Collector + Grafana |
| `examples/kubernetes/` | Kubernetes manifests: Deployment, ConfigMap, Service, HPA |

**Quality bar for examples:** Every example must compile (`cargo check --examples`), run without external dependencies beyond Postgres (using `testcontainers-rs` for integration examples), and be referenced from the README.

---

### Migration Guide

The README includes a brief "coming from X" orientation (Apalis, hand-rolled Tokio+SQLx queues). A full migration runbook covering parallel deployment and drain-and-cutover patterns is post-capstone scope.

## Project Scoping & Phased Development

### MVP Strategy & Philosophy

**MVP Approach:** Platform MVP — build the durable execution foundation correctly, minimally, and demonstrably. The success signal is not feature breadth but correctness: a task submitted to iron-defer will execute at least once, and the system will recover and retry it on failure. Everything in MVP scope exists to prove and demonstrate that guarantee.

**Capstone boundary:** The MVP is the capstone project. Post-MVP phases represent iron-defer's open-source roadmap beyond the capstone. Phase 2 and Phase 3 are documented for architectural continuity but are outside capstone scope.

**Resource Requirements:** Single Rust engineer. 4-week delivery horizon. No external services beyond PostgreSQL.

---

### MVP Feature Set (Phase 1)

**Core User Journeys Supported:**

| Journey | MVP Support |
|---|---|
| Rust Engineer — Happy Path | Full |
| Rust Engineer — Edge Case (failure & recovery) | Full |
| Platform Engineer (Kubernetes / ops) | Full |
| Architect / Compliance Evaluator | Full |
| API Consumer (non-Rust REST) | Full |
| Telecom BSS Orchestrator v1 | Full |
| Migrating Engineer | Full |
| Telecom BSS Orchestrator v2 | Deferred (Growth) |
| OSS Contributor | Deferred (Vision) |

**Must-Have Capabilities:**

The 4-week delivery breakdown is defined in *§ Product Scope — MVP*. Key scoping note for the capstone: the **chaos integration test suite** (`testcontainers-rs`) is a first-class Week 4 deliverable, not optional — it is the machine-verifiable proof of the correctness guarantee.

**Explicit MVP Exclusions:**

- Task authorization / access control (authN/authZ on REST API) — single-tenant, network-isolated deployment assumed
- Transactional enqueue (River pattern) — deferred to Growth
- Full OTel 4-pillar (Traces + Events) — MVP delivers Metrics + Logs; traces deferred to Growth
- Append-only audit log table — Postgres tasks table serves as audit trail for MVP; dedicated log table deferred to Growth
- Zero-downtime engine upgrade — MVP documents drain-and-restart procedure

---

### Growth Feature Set (Phase 2)

**Tier 1 — Production Adoption Enablers:**

| Feature | Key Capability | Dependencies |
|---|---|---|
| G1: Idempotency keys | Exactly-once submission via client-supplied key | Sweeper (retention cleanup) |
| G2: Transactional enqueue | Atomic enqueue inside caller's DB transaction | None |

**Tier 2 — Observability, Compliance & Performance:**

| Feature | Key Capability | Dependencies |
|---|---|---|
| G4: OTel traces + events | W3C trace propagation, structured lifecycle events | MVP OTel infra |
| G5: Audit log table | Append-only, tamper-evident transition history | G4 (trace_id), G3 (mutual exclusion) |
| G3: UNLOGGED table mode | ≥5× throughput for non-durable workloads | FR40 (mutual exclusion, MVP) |

**Tier 3 — Differentiated Capabilities:**

| Feature | Key Capability | Dependencies |
|---|---|---|
| G6: Checkpoint/resume | Mid-execution state persistence, crash recovery | None |
| G7: HITL suspend/resume | External signal-driven task resumption | G6 (state persistence pattern) |
| G8: Geographic pinning | Region-labeled task routing to matching workers | None |

**Sequencing rationale:** Tier 1 ships first because it removes adoption blockers (duplicate submissions, dual-write bugs) with minimal schema changes. Tier 2 builds on Tier 1 and adds observability/compliance infrastructure; within Tier 2, G4 (traces) must ship before or with G5 (audit log) so the audit schema includes `trace_id`. G3 (UNLOGGED) is in Tier 2 because it serves a specific performance segment, not general adoption. Tier 3 introduces new execution semantics; G6 (checkpoint) is a hard blocking dependency for G7 (HITL suspend). G8 (geographic pinning) is independent. Recommended implementation order: G1 → G2 → G4 → G5 → G3 → G6 → G8 → G7.

**Growth-Phase Journeys Supported:**

| Journey | Growth Support |
|---|---|
| Rust Engineer — Idempotent Submission | Full (Tier 1) |
| Platform Engineer — Audit & Compliance | Full (Tier 2) |
| AI Platform Engineer — Checkpoint/Resume | Full (Tier 3) |
| Telecom BSS Orchestrator v2 | Full (Tier 3: G6 + G7) |

---

### Risk Mitigation Strategy

**Technical Risks — Correctness First:**

The primary risk is lease management correctness: a task claimed but not completed, a lease that doesn't expire cleanly, a Sweeper that recovers too aggressively or not at all. Mitigation is architectural: the `SKIP LOCKED` claiming engine is the correctness primitive, not a performance optimization. The chaos test suite (Week 4) is a first-class deliverable precisely because it makes correctness claims falsifiable. Every correctness invariant has a corresponding test assertion.

Secondary risk: bounded connection pool enforcement. Unbounded growth cascades into Postgres `max_connections` exhaustion. Mitigation: pool size is a required construction parameter for embedded mode (rejected at `IronDefer::new()` if absent), a required env var in standalone mode. Pool metrics emitted as OTel gauges.

**Market Risks:**

Adoption risk in OSS is real but structurally mitigated — the Rust durable execution gap is documented and unoccupied. The primary market risk is timing: if Apalis ships a stable 1.0 with durable semantics before iron-defer, the gap closes. Mitigation: ship MVP, publish to crates.io, document the correctness guarantees clearly. Being second but more correct is still a winning position.

**Resource Risks:**

Solo engineer, 4-week timeline. If scope must compress further, the priority order is: (1) correctness of the claiming engine and Sweeper, (2) embedded library API, (3) standalone binary + Docker, (4) CLI, (5) full OTel metrics coverage. The REST API and chaos tests are non-negotiable — they are the demonstration artifacts for the capstone and the trust-building artifacts for OSS adoption.

## Functional Requirements

### Task Submission

- **FR1:** A Rust developer can submit a task to a named queue via the embedded library API with a serialized payload
- **FR2:** An operator can submit a task to a named queue via the CLI with a JSON payload
- **FR3:** An external service can submit a task to a named queue via the REST API with a JSON payload
- **FR4:** A developer can configure a scheduled execution time for a task at submission
- **FR5:** A developer can define and submit tasks across multiple independently configured named queues

### Task Execution

- **FR6:** The engine can claim pending tasks from a queue exclusively — exactly one worker claims each task, with no duplicate claiming under concurrent access
- **FR7:** A Rust developer can define task execution logic by implementing a standard engine contract (the `Task` trait)
- **FR8:** A developer can register task type handlers with the engine for dispatch at runtime
- **FR9:** The engine can execute tasks concurrently up to a configurable per-queue concurrency limit
- **FR10:** The engine can record task completion (success or failure) back to the task store atomically

### Task Resilience

- **FR11:** The engine can automatically retry a failed task up to a configurable maximum attempt count
- **FR12:** The engine can apply configurable backoff between retry attempts
- **FR13:** The engine can recover tasks stuck in Running state beyond their configured lease expiry
- **FR14:** The engine can release task leases and drain in-flight tasks on receiving a shutdown signal
- **FR15:** A developer can configure per-queue lease duration and Sweeper recovery interval
- **FR16:** The engine can reconnect to Postgres automatically after a connection loss without dropping pending tasks
- **FR43:** The engine can transition a task to a distinct terminal failure state when maximum retry attempts are exhausted, distinguishable from a transiently failed task

### Observability

- **FR17:** The engine can emit queue depth, execution latency, retry rate, and failure rate metrics via OTel OTLP export
- **FR18:** The engine can expose accumulated metrics in Prometheus text format via a scrape endpoint
- **FR19:** The engine can emit a structured log record for every task lifecycle event tagged with `task_id`, `queue_name`, `worker_id`, and `attempt_number`
- **FR20:** The engine can emit connection pool utilization metrics (available connections, in-use, wait queue depth)
- **FR21:** An operator can query every task state transition (Pending, Running, Completed, Failed, Cancelled) with timestamps and worker identity from the task store using standard SQL
- **FR44:** The engine can emit a metric when a task reaches terminal failure, enabling operators to configure downstream alerting

### Operator Interface

- **FR22:** An operator can inspect tasks in a queue via the CLI with filtering by status
- **FR23:** An operator can inspect active worker status via the CLI
- **FR24:** An operator can validate the engine configuration via the CLI before starting
- **FR25:** An external service can query the current status and result of a specific task by task ID via the REST API
- **FR26:** An external service can list tasks with filtering by queue name and status via the REST API
- **FR27:** An external service can cancel a pending task via the REST API
- **FR28:** An external service can query the list of registered queues along with their current depth and active worker statistics via the REST API
- **FR29:** An operator or monitoring system can verify engine liveness and Postgres connectivity via dedicated HTTP probe endpoints
- **FR30:** An API consumer can discover the REST API contract via an embedded OpenAPI specification

### Deployment & Distribution

- **FR31:** A Rust developer can add iron-defer as a Cargo dependency and embed the engine in an existing Tokio application without provisioning new infrastructure
- **FR32:** An operator can deploy the standalone binary as a Docker container using a published image and provided Docker Compose manifest
- **FR33:** An operator can deploy the standalone binary to Kubernetes using provided deployment manifests
- **FR34:** An operator can configure the standalone binary entirely via environment variables
- **FR35:** A developer can enable or disable optional capabilities (metrics, tracing, audit-log, unlogged) via Cargo feature flags
- **FR36:** A developer can run reference examples from the repository that execute without external dependencies beyond Postgres

### Compliance & Privacy

- **FR37:** The engine can record every task state transition with timestamps and worker identity in a queryable task store
- **FR38:** The engine can suppress task payload content from all log and metric output by default
- **FR39:** A developer can explicitly opt in to payload inclusion in log output via configuration
- **FR40:** The engine can enforce mutual exclusion between UNLOGGED table mode and audit logging, rejecting startup with an explicit error if both are configured
- **FR41:** The engine can enforce a maximum Postgres connection pool size, rejecting construction if the configured size exceeds a documented ceiling
- **FR42:** The engine ships an integration test suite that produces machine-verifiable OTel signal evidence for every task lifecycle event, usable as compliance audit proof

### Growth — Exactly-Once Submission (G1)

- **FR45:** A developer can supply an optional idempotency key when submitting a task; a second submission with the same key and queue returns the existing task (HTTP 200, not 201) instead of creating a duplicate; concurrent duplicate submitters each receive the existing task, never a 500 error
- **FR46:** The engine can enforce idempotency key uniqueness at the database level — active tasks hold their key exclusively, terminal tasks release their keys automatically
- **FR47:** An operator can configure a key retention window (`idempotency_key_retention`, default 24h) after which the Sweeper cleans up expired idempotency keys; cleanup piggybacks on the existing Sweeper tick (no new background actor)

### Growth — Transactional Enqueue (G2)

- **FR48:** A developer can enqueue a task inside a caller-provided database transaction, with the task becoming visible to workers only when the transaction commits
- **FR49:** A task enqueued inside a rolled-back transaction produces zero visible tasks in the queue

### Growth — UNLOGGED Table Mode (G3)

- **FR50:** An operator can configure the engine to use Postgres UNLOGGED tables for the tasks table via a configuration flag
- **FR51:** The engine can create the appropriate table type (UNLOGGED or standard) at startup based on the `unlogged_tables` configuration flag

### Growth — OTel Traces & Events (G4)

- **FR52:** The engine can create an OTel span for each task execution with `task_id`, `queue`, `kind`, and `attempt` attributes
- **FR53:** The engine can propagate a W3C `traceparent` context supplied at enqueue time through to the worker execution span
- **FR54:** The engine can emit OTel Events for every task state transition with structured attributes

### Growth — Audit Log (G5)

- **FR55:** The engine can record every task state transition as an immutable row in a dedicated `task_audit_log` table within the same database transaction as the state change
- **FR56:** An operator can query the complete lifecycle of any task from the audit log table using standard SQL

### Growth — Checkpoint/Resume (G6)

- **FR57:** A task handler can persist checkpoint data during execution via the `TaskContext`, recoverable on retry
- **FR58:** On retry after a crash, a task handler can retrieve the most recent checkpoint data from the `TaskContext`
- **FR59:** Checkpoint data is cleared automatically on task completion

### Growth — HITL Suspend/Resume (G7)

- **FR60:** A task handler can suspend execution, transitioning the task to `Suspended` status and yielding its worker slot; the handler must checkpoint state via G6 before suspending
- **FR61:** An external caller can resume a suspended task via `POST /tasks/{id}/signal` with an optional JSON payload; concurrent signals to the same task result in exactly one success (200) and the rest receive 409
- **FR62:** Suspended tasks do not count against the queue's concurrency limit and do not trigger the Sweeper's zombie recovery
- **FR63:** A suspend watchdog (piggybacking on the existing Sweeper tick) auto-fails tasks that remain in `Suspended` status beyond a configurable `suspend_timeout` (default 24h)

### Growth — Geographic Worker Pinning (G8)

- **FR64:** A developer can submit a task with an optional region label that restricts which workers can claim it
- **FR65:** A worker configured with a region label claims only tasks matching its region or tasks with no region label
- **FR66:** Region labels are exposed in queue statistics and OTel metric labels

## Non-Functional Requirements

### Performance

- **NFR-P1:** Task claiming latency (time from `Pending` to `Running`) must be ≤ 100ms at p99 under normal load on a single Postgres instance, at queue depths up to 1 million pending tasks
- **NFR-P2:** The engine must sustain ≥ 10,000 task completions per second on a single commodity Postgres instance, as measured by the included benchmark suite
- **NFR-P3:** The Fetcher loop must impose < 1ms per poll cycle overhead when the queue is empty, as measured by Criterion microbenchmark of a single poll iteration against an empty queue on the reference benchmark environment
- **NFR-P4:** Engine initialization in embedded mode must not block the Tokio runtime for more than 500ms (threshold aligned with default Kubernetes readiness probe timeout)

### Security

- **NFR-S1:** All Postgres connections must support TLS encryption; TLS is the documented default recommendation for production deployments, verified by an integration test that connects with `sslmode=require` and rejects `sslmode=disable` in production configuration profiles
- **NFR-S2:** Task payload content must not appear in log output, OTel traces, or emitted metrics by default (`log_payload: false` is the out-of-the-box configuration)
- **NFR-S3:** The standalone binary must make no outbound network calls other than to the configured Postgres instance and OTel Collector endpoint; no telemetry, no license checks, no undocumented external dependencies; verified by `cargo deny` audit of dependency tree and a network-isolated integration test (firewall all egress except Postgres port)
- **NFR-S4:** Task payload data must not be retained in memory beyond the execution lifetime of the task that owns it; verified by code review confirming `Arc<Value>` is the sole owner and drops when the worker dispatch completes (no static caches, no global buffers)

### Scalability

- **NFR-SC1:** Multiple engine instances sharing a single Postgres queue table must coordinate purely via database primitives — no external coordination service required; throughput must scale within 80% of linear with worker count (≥ 0.8× per additional worker) up to the Postgres connection ceiling, as measured by the Criterion benchmark suite with 1, 2, 4, and 8 concurrent workers
- **NFR-SC2:** The tasks table schema must not use Postgres features incompatible with standard table partitioning or Postgres-compatible distributed databases (Aurora, Citus); specifically: no advisory locks in schema, no unlogged sequences, no custom types beyond standard SQL types; verified by schema review checklist and `pg_dump` diff against a vanilla Postgres 14+ instance
- **NFR-SC3:** Worker instances must be stateless with respect to task routing — any worker can claim any task from its registered queues; no sticky routing, no worker-specific state; verified by chaos integration test: kill worker A mid-execution, worker B claims and completes the same task after Sweeper recovery
- **NFR-SC4:** The claiming engine must not produce lock contention that causes worker starvation — every active worker must successfully claim a task within one poll cycle when tasks are available; verified by load test: 8 workers, 1000 pending tasks, assert zero workers idle for > 2 consecutive poll cycles

### Reliability

- **NFR-R1:** Tasks in `Pending` or `Running` state at the time of a Postgres outage must execute exactly once after recovery — zero task loss; tasks submitted *during* the outage are expected to fail at the call site and are the caller's responsibility
- **NFR-R2:** The Sweeper must recover all zombie tasks (Running + expired lease) within 2× the configured Sweeper interval; verified by the chaos integration test suite in CI
- **NFR-R3:** A worker receiving SIGTERM must complete in-flight tasks or release their leases within the configured `termination_grace_period` (default: 60s); no orphaned `Running` tasks on graceful shutdown
- **NFR-R4:** The chaos integration test suite must pass with zero task loss and zero duplicate completions on every CI run; flaky chaos tests are treated as blocking failures
- **NFR-R5:** The full chaos integration test suite must complete within 10 minutes on standard CI hardware (2 vCPU, 8GB RAM)
- **NFR-R6:** When the connection pool is fully saturated, the engine must emit a warning-level log and surface backlog depth via the `pool_wait_queue_depth` metric; task claiming must never silently fail or drop tasks

### Integration

- **NFR-I1:** Metrics and logs must be emitted via the OpenTelemetry SDK with OTLP/HTTP export supported by default; OTLP/gRPC support is available via a Cargo feature flag
- **NFR-I2:** The Prometheus scrape endpoint must emit metrics in Prometheus text exposition format ≥ 0.0.4, compatible with any Prometheus-compatible scraper without custom configuration
- **NFR-I3:** The REST API must be documented via an OpenAPI 3.x specification generated from the API code (not hand-maintained), embedded in the binary, and served at a stable endpoint
- **NFR-I4:** The engine must accept standard libpq-compatible `DATABASE_URL` connection strings and pre-constructed `sqlx::PgPool` instances interchangeably

### Maintainability

- **NFR-M1:** The MSRV (Minimum Supported Rust Version) must be declared in `Cargo.toml` and enforced in CI; any MSRV bump requires a minor version increment at minimum
- **NFR-M2:** The engine must not execute malformed SQL queries against the database at runtime; SQLx compile-time query verification (`query!` / `query_as!` macros) is the required mechanism — runtime SQL generation is forbidden in the core engine
- **NFR-M3:** The public API surface (`Task` trait, `IronDefer` builder, `TaskContext`) must follow semantic versioning; breaking changes to the public API require a major version increment
- **NFR-M4:** The core engine crate must not introduce transitive dependencies beyond the allowlist: Tokio, SQLx, serde/serde_json, chrono, uuid, thiserror, OTel SDK (opentelemetry, opentelemetry_sdk, opentelemetry-prometheus, prometheus), axum, clap, figment, tracing; verified by `cargo tree -p iron-defer-domain` and CI `cargo deny` check

### Usability

- **NFR-U1:** A developer with an existing Rust + Postgres project must be able to submit their first durable task within 30 minutes using only the README; verified by the `basic_enqueue` example compiling and completing against a fresh Postgres instance with zero configuration beyond `DATABASE_URL`
- **NFR-U2:** A minimal `Task` trait implementation — including struct definition, trait impl, and engine registration — must require no more than 15 lines of Rust; verified by `examples/basic_task.rs`

### Growth — Reliability Extensions

- **NFR-R7:** Idempotency key enforcement must add < 5ms latency to task submission at p99 on the reference benchmark environment (single Postgres instance, same hardware as NFR-P2 baseline), as measured by the Criterion benchmark suite using a ratio comparison against the non-idempotent submission baseline from the same test run
- **NFR-R8:** Transactional enqueue must not extend the caller's transaction duration by more than 10ms at p99 on the reference benchmark environment; the engine must not acquire additional locks or perform additional queries beyond the single `INSERT INTO tasks`
- **NFR-R9:** Checkpoint persistence must complete within 50ms at p99 on the reference benchmark environment for checkpoint payloads up to 1 MiB, as measured by the Criterion benchmark suite

### Growth — Compliance Extensions

- **NFR-C1:** The audit log table must be append-only — enforced at the database level by a `BEFORE UPDATE OR DELETE` trigger on `task_audit_log` that raises an exception; additionally, zero `UPDATE` or `DELETE` queries against `task_audit_log` in the application codebase, verifiable by grep
- **NFR-C2:** Audit log writes must be atomic with the state transition they record — executed in the same database transaction on the same connection; a committed state change without a corresponding audit row is a system failure, verifiable by a fault-injection integration test (kill connection between state INSERT and audit INSERT)
- **NFR-C3:** W3C trace context propagation must preserve the original trace ID across at least 3 retry attempts, verifiable by integration test using an in-memory span exporter (preferred) or OTLP collector testcontainer

### Growth — Scalability Extensions

- **NFR-SC5:** Geographic worker pinning must not degrade claiming throughput by more than 10% compared to unpinned mode at p99 on the reference benchmark environment, as measured by the Criterion benchmark suite with 4 region labels and proportional task distribution
- **NFR-SC6:** UNLOGGED table mode must deliver ≥ 5× throughput improvement over WAL-logged mode on production-configured Postgres (tuned `shared_buffers`, `max_wal_size`, `checkpoint_completion_target`) on dedicated hardware; this benchmark runs outside CI on a scheduled basis, not per-PR

