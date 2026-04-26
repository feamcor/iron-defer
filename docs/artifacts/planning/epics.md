---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
inputDocuments:
  - docs/artifacts/planning/prd.md
  - docs/artifacts/planning/architecture.md
  - docs/artifacts/planning/sprint-change-proposal-2026-04-26.md
---

# iron-defer - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for iron-defer, decomposing the requirements from the PRD, Architecture, and approved sprint change proposal into implementable stories.

## Requirements Inventory

### Functional Requirements

FR1: A Rust developer can submit a task to a named queue via the embedded library API with a serialized payload.
FR2: An operator can submit a task to a named queue via the CLI with a JSON payload.
FR3: An external service can submit a task to a named queue via the REST API with a JSON payload.
FR4: A developer can configure a scheduled execution time for a task at submission.
FR5: A developer can define and submit tasks across multiple independently configured named queues.
FR6: The engine can claim pending tasks from a queue exclusively with no duplicate claiming under concurrent access.
FR7: A Rust developer can define task execution logic by implementing the `Task` trait.
FR8: A developer can register task type handlers with the engine for dispatch at runtime.
FR9: The engine can execute tasks concurrently up to a configurable per-queue concurrency limit.
FR10: The engine can record task completion (success or failure) back to the task store atomically.
FR11: The engine can automatically retry a failed task up to a configurable maximum attempt count.
FR12: The engine can apply configurable backoff between retry attempts.
FR13: The engine can recover tasks stuck in Running state beyond their configured lease expiry.
FR14: The engine can release task leases and drain in-flight tasks on receiving a shutdown signal.
FR15: A developer can configure per-queue lease duration and Sweeper recovery interval.
FR16: The engine can reconnect to Postgres automatically after a connection loss without dropping pending tasks.
FR17: The engine can emit queue depth, execution latency, retry rate, and failure rate metrics via OTel OTLP export.
FR18: The engine can expose accumulated metrics in Prometheus text format via a scrape endpoint.
FR19: The engine can emit structured logs for task lifecycle events with task and worker correlation fields.
FR20: The engine can emit connection pool utilization metrics.
FR21: An operator can query task state transitions with timestamps and worker identity from the task store using SQL.
FR22: An operator can inspect tasks in a queue via the CLI with filtering by status.
FR23: An operator can inspect active worker status via the CLI.
FR24: An operator can validate the engine configuration via the CLI before starting.
FR25: An external service can query status and result of a specific task by task ID via REST.
FR26: An external service can list tasks with filtering by queue name and status via REST.
FR27: An external service can cancel a pending task via REST.
FR28: An external service can query registered queues with current depth and active worker statistics via REST.
FR29: An operator or monitoring system can verify liveness and Postgres connectivity via HTTP probes.
FR30: An API consumer can discover the REST API contract via embedded OpenAPI spec.
FR31: A Rust developer can add iron-defer as a Cargo dependency and embed the engine without provisioning new infrastructure.
FR32: An operator can deploy the standalone binary as a Docker container using published image and compose manifest.
FR33: An operator can deploy the standalone binary to Kubernetes using provided manifests.
FR34: An operator can configure the standalone binary entirely via environment variables.
FR35: A developer can enable or disable optional capabilities via Cargo feature flags.
FR36: A developer can run reference examples that execute without external dependencies beyond Postgres.
FR37: The engine can record task state transitions with timestamps and worker identity in a queryable store.
FR38: The engine can suppress task payload content from logs and metrics by default.
FR39: A developer can opt in to payload inclusion in logs via configuration.
FR40: The engine enforces mutual exclusion between UNLOGGED mode and audit logging with explicit startup error.
FR41: The engine enforces a maximum Postgres connection pool size with documented ceiling.
FR42: The engine ships an integration test suite producing machine-verifiable OTel lifecycle evidence for compliance.
FR43: The engine transitions a task to a distinct terminal failure state when retries are exhausted.
FR44: The engine emits a metric when a task reaches terminal failure for alerting.
FR45: Optional idempotency key submission returns existing task on duplicates with HTTP 200 semantics.
FR46: Idempotency key uniqueness is enforced at DB level for active tasks.
FR47: Sweeper cleans expired idempotency keys after retention window.
FR48: Tasks can be enqueued inside caller-provided DB transaction and become visible only on commit.
FR49: Rolled-back transactional enqueue produces zero visible tasks.
FR50: Operator can configure UNLOGGED table mode.
FR51: Engine creates table type based on `unlogged_tables` flag.
FR52: Engine creates OTel span per task execution with required attributes.
FR53: Engine propagates W3C `traceparent` from enqueue to execution span.
FR54: Engine emits OTel events for task state transitions.
FR55: Engine writes immutable audit rows in same transaction as state transition.
FR56: Operator can query complete task lifecycle in audit log table.
FR57: Handler can persist checkpoint data during execution.
FR58: Handler can retrieve latest checkpoint data on retry.
FR59: Checkpoint data is cleared automatically on task completion.
FR60: Handler can suspend task with checkpoint-before-suspend behavior.
FR61: External signal endpoint resumes suspended task; concurrent signals produce one success.
FR62: Suspended tasks do not consume concurrency and are excluded from zombie recovery.
FR63: Suspend watchdog auto-fails overdue suspended tasks.
FR64: Developer can submit tasks with optional region label for claim restriction.
FR65: Region-configured workers claim matching-region or unpinned tasks.
FR66: Region labels are exposed in queue statistics and OTel metric labels.

### NonFunctional Requirements

NFR-P1: Claiming latency is <= 100ms at p99 under defined load.
NFR-P2: Throughput sustains >= 10,000 completions per second on reference setup.
NFR-P3: Empty-queue poll overhead is < 1ms per cycle.
NFR-P4: Embedded initialization does not block runtime for more than 500ms.
NFR-S1: Postgres connections support TLS and production profiles enforce secure posture.
NFR-S2: Payload content is excluded from logs, traces, and metrics by default.
NFR-S3: Standalone binary makes no outbound calls except Postgres and configured OTel collector.
NFR-S4: Payload memory is not retained beyond task execution lifetime.
NFR-SC1: Multi-instance coordination scales near-linearly up to connection ceiling.
NFR-SC2: Schema remains compatible with partitioning and Postgres-compatible distributed options.
NFR-SC3: Workers are stateless for routing and recoverability across node loss.
NFR-SC4: Claiming avoids starvation and lock-contention-induced worker idling.
NFR-R1: No task loss for pre-existing pending/running tasks across Postgres outage.
NFR-R2: Sweeper recovers zombie tasks within 2x sweeper interval.
NFR-R3: SIGTERM drain/releases complete within configured grace period.
NFR-R4: Chaos suite is non-flaky and blocking in CI.
NFR-R5: Full chaos suite completes within CI time budget.
NFR-R6: Pool saturation produces warning signal and backlog visibility without silent drops.
NFR-I1: OTel SDK emission supports OTLP/HTTP by default with optional OTLP/gRPC feature.
NFR-I2: Prometheus scrape format is standards-compatible.
NFR-I3: OpenAPI 3.x spec is code-generated, embedded, and served at stable endpoint.
NFR-I4: Engine accepts DATABASE_URL and pre-built PgPool interchangeably.
NFR-M1: MSRV is declared and CI-enforced; bumps follow semver policy.
NFR-M2: Core engine avoids malformed runtime SQL through verified SQLx usage policy.
NFR-M3: Public API follows semantic versioning.
NFR-M4: Dependency footprint remains within documented allowlist and deny checks.
NFR-U1: Time-to-first-task is <= 30 minutes from README-only onboarding.
NFR-U2: Minimal task implementation remains concise and ergonomic.
NFR-R7: Idempotency adds < 5ms submission latency at p99.
NFR-R8: Transactional enqueue adds <= 10ms to caller transaction at p99.
NFR-R9: Checkpoint persistence completes within 50ms p99 for payloads up to 1 MiB.
NFR-C1: Audit log is append-only with DB-level immutability enforcement.
NFR-C2: Audit writes are atomic with state transitions.
NFR-C3: W3C trace context is preserved across retries.
NFR-SC5: Geographic pinning degrades throughput by less than 10% at p99.
NFR-SC6: UNLOGGED mode delivers >= 5x throughput on production-like benchmark setup.

### Additional Requirements

- Starter template requirement: use manual Cargo workspace initialization with 4-crate hexagonal layout (`domain`, `application`, `infrastructure`, `api`).
- Rust 2024 edition and declared MSRV policy must remain enforced in workspace and CI.
- Preserve strict architectural boundaries: domain <- application <- infrastructure <- api wiring.
- Embedded library must accept caller-provided `PgPool` and must not create its own Tokio runtime.
- Standalone mode must wire graceful shutdown explicitly through axum `with_graceful_shutdown`.
- SQLx offline cache and migration strategy are mandatory build/CI constraints.
- Public API and REST contract compatibility are release-sensitive and require explicit change control.
- No-auth MVP boundary remains documented as deployment security assumption (network-isolated/private).
- OTel metrics/logs/traces strategy must stay aligned with exporter and backend compatibility requirements.
- Chaos and integration tests are first-class acceptance artifacts, not optional diagnostics.
- Release quality gate from approved sprint change proposal: feature work is not release-complete while critical review findings remain unresolved or un-waived.
- Add release-hardening execution scope (Epic 13) for unresolved findings in stories 9.1, 10.1, 12.2, and 12.3.
- Queue stats API behavior must remain backward-compatible unless a versioned or explicit opt-in mode is introduced.
- Region metric labels require bounded/normalized cardinality strategy for observability safety.
- Tracking artifacts (`sprint-status.yaml`, story states) must remain synchronized with review closure evidence.

### UX Design Requirements

N/A - No UX design document found. This project is backend infrastructure.

### FR Coverage Map

FR1: Epic 1 - Embedded task submission baseline.
FR2: Epic 4 - CLI operator submission workflow.
FR3: Epic 4 - REST task submission for external clients.
FR4: Epic 1 - Scheduled submission support.
FR5: Epic 2 - Multi-queue execution behavior.
FR6: Epic 2 - Atomic exclusive task claiming.
FR7: Epic 1 - Task trait implementation contract.
FR8: Epic 1 - Runtime handler registration.
FR9: Epic 2 - Configurable worker concurrency.
FR10: Epic 2 - Atomic completion/failure persistence.
FR11: Epic 2 - Retry-attempt mechanics.
FR12: Epic 2 - Backoff policy application.
FR13: Epic 3 - Zombie recovery by sweeper.
FR14: Epic 3 - Graceful shutdown lease handling.
FR15: Epic 3 - Lease/sweeper interval configuration.
FR16: Epic 3 - Postgres reconnect resilience.
FR17: Epic 5 - OTel metrics emission.
FR18: Epic 5 - Prometheus scrape endpoint exposure.
FR19: Epic 5 - Structured lifecycle logging.
FR20: Epic 5 - Pool utilization metrics.
FR21: Epic 5 - SQL-queryable transition evidence.
FR22: Epic 4 - CLI task inspection.
FR23: Epic 4 - CLI worker status inspection.
FR24: Epic 4 - CLI config validation.
FR25: Epic 4 - REST task status lookup.
FR26: Epic 4 - REST task listing/filtering.
FR27: Epic 4 - REST cancellation endpoint.
FR28: Epic 5 - Queue depth/worker stats API.
FR29: Epic 5 - Liveness/readiness probes.
FR30: Epic 4 - OpenAPI discovery endpoint.
FR31: Epic 1 - Embeddable library onboarding.
FR32: Epic 6 - Docker deployment pathway.
FR33: Epic 6 - Kubernetes deployment pathway.
FR34: Epic 6 - Env-var-based standalone configuration.
FR35: Epic 6 - Feature-flag capability control.
FR36: Epic 6 - Runnable reference examples.
FR37: Epic 7 - Queryable transition record baseline.
FR38: Epic 7 - Payload privacy default behavior.
FR39: Epic 7 - Explicit payload logging opt-in.
FR40: Epic 7 - UNLOGGED/audit mutual exclusion enforcement.
FR41: Epic 3 - Pool size safety ceiling enforcement.
FR42: Epic 5 - Compliance-grade OTel evidence tests.
FR43: Epic 2 - Distinct terminal failure state.
FR44: Epic 5 - Terminal-failure alerting metric.
FR45: Epic 8 - Idempotency duplicate-submission semantics.
FR46: Epic 8 - DB-enforced active-key uniqueness.
FR47: Epic 8 - Sweeper key-retention cleanup.
FR48: Epic 8 - Transactional enqueue API.
FR49: Epic 8 - Rollback visibility guarantees.
FR50: Epic 10 - UNLOGGED mode configuration.
FR51: Epic 10 - Conditional table creation behavior.
FR52: Epic 9 - Task-execution span creation.
FR53: Epic 9 - W3C trace-context propagation.
FR54: Epic 9 - OTel transition-event emission.
FR55: Epic 9 - Atomic append-only audit writes.
FR56: Epic 9 - Audit lifecycle querying.
FR57: Epic 11 - Checkpoint persistence API.
FR58: Epic 11 - Checkpoint resume retrieval.
FR59: Epic 11 - Checkpoint clear-on-complete behavior.
FR60: Epic 12 - Suspend with checkpoint-first rule.
FR61: Epic 12 - Signal-based resume with race semantics.
FR62: Epic 12 - Suspended-task concurrency behavior.
FR63: Epic 12 - Suspend-timeout watchdog behavior.
FR64: Epic 12 - Region-labeled submission support.
FR65: Epic 12 - Region-aware claiming policy.
FR66: Epic 12 - Region visibility in stats/metrics.

## Epic List

### Epic 1: Foundation and First Durable Task
A Rust developer can embed iron-defer and run a first durable task end-to-end with validated registration and scheduling basics.
**FRs covered:** FR1, FR4, FR7, FR8, FR31

### Epic 2: Core Execution and Queue Processing
A developer can run durable multi-queue execution with atomic claiming, retries, and terminal failure handling.
**FRs covered:** FR5, FR6, FR9, FR10, FR11, FR12, FR43

### Epic 3: Recovery and Runtime Resilience
An operator can trust service behavior through outages and shutdowns with lease safety and reconnect recovery.
**FRs covered:** FR13, FR14, FR15, FR16, FR41

### Epic 4: Operator Interfaces (REST and CLI)
Operators and external systems can submit, inspect, list, cancel, and discover API behavior through usable interfaces.
**FRs covered:** FR2, FR3, FR22, FR23, FR24, FR25, FR26, FR27, FR30

### Epic 5: Observability and Health Visibility
Operators can monitor queue/system health and produce compliance-grade lifecycle evidence.
**FRs covered:** FR17, FR18, FR19, FR20, FR21, FR28, FR29, FR42, FR44

### Epic 6: Deployment and Distribution
Teams can deploy, configure, and evaluate iron-defer in Docker/Kubernetes and example-driven workflows.
**FRs covered:** FR32, FR33, FR34, FR35, FR36

### Epic 7: Compliance and Privacy Baseline
Teams can operate with safe-by-default payload privacy and compliance guardrails in MVP behavior.
**FRs covered:** FR37, FR38, FR39, FR40

### Epic 8: Growth Tier 1 - Submission Safety
Developers get idempotent and transactional submission guarantees for adoption-critical correctness.
**FRs covered:** FR45, FR46, FR47, FR48, FR49

### Epic 9: Growth Tier 2 - Trace and Audit Evidence
Operators gain trace propagation and append-only audit evidence for compliance and diagnostics.
**FRs covered:** FR52, FR53, FR54, FR55, FR56

### Epic 10: Growth Tier 2 - Performance Mode
Operators can opt into UNLOGGED performance mode with explicit safety constraints.
**FRs covered:** FR50, FR51

### Epic 11: Growth Tier 3 - Checkpoint and Recovery Workflows
Developers can persist and resume multi-step workflow state safely across retries and crashes.
**FRs covered:** FR57, FR58, FR59

### Epic 12: Growth Tier 3 - HITL and Data Residency
Teams can suspend/resume workflows and enforce geographic worker pinning for residency-sensitive workloads.
**FRs covered:** FR60, FR61, FR62, FR63, FR64, FR65, FR66

### Epic 13: Release Cleanup and Readiness Gate
The team closes critical unresolved findings, enforces compatibility/observability safeguards, and gates release on auditable closure.
**FRs covered:** Cross-cutting release-readiness closure for FR45-FR66 quality expectations and approved change-proposal constraints.

## Epic 1: Foundation and First Durable Task

A Rust developer can embed iron-defer and run a first durable task end-to-end with validated registration and scheduling basics.

### Story 1.1: Workspace and Embedded Engine Bootstrap

As a Rust developer,
I want a compile-ready embedded iron-defer workspace and builder bootstrap,
So that I can integrate durable task execution into an existing Tokio app without extra infrastructure.

**Acceptance Criteria:**

**Given** a fresh checkout of the project
**When** workspace checks are run
**Then** the workspace compiles with the 4-crate architecture (`domain`, `application`, `infrastructure`, `api`)
**And** crate boundaries follow documented architecture constraints

**Given** the embedded API surface
**When** the engine is constructed via builder with a caller-provided `PgPool`
**Then** the library does not create its own Tokio runtime
**And** engine construction succeeds with documented defaults

**Given** configuration and migration setup
**When** the embedded engine initializes in local development
**Then** required migrations are available through the embedded migrator path
**And** startup fails with a clear typed error for invalid configuration

### Story 1.2: Task Registration and First Successful Execution

As a Rust developer,
I want to register a task handler and execute a submitted task successfully,
So that I can validate the core embedded integration path in a real app flow.

**Acceptance Criteria:**

**Given** a task type implementing the `Task` trait
**When** it is registered with the engine registry before startup
**Then** worker dispatch resolves the task kind to the correct handler
**And** missing registration fails with a clear runtime error

**Given** a running embedded engine and a valid payload
**When** a task is submitted to a queue through the library API
**Then** the task transitions through `Pending -> Running -> Completed`
**And** completion is persisted and queryable via task lookup

**Given** a malformed payload for a registered task kind
**When** the worker attempts deserialization and dispatch
**Then** the task is marked failed with typed error context
**And** no panic escapes the worker execution loop

### Story 1.3: Scheduled Submission Baseline

As a Rust developer,
I want to submit tasks with a scheduled execution time,
So that delayed work can be queued without custom timing infrastructure.

**Acceptance Criteria:**

**Given** a task submitted with `scheduled_at` in the future
**When** workers poll the queue before that timestamp
**Then** the task is not claimed
**And** it remains in `Pending` state

**Given** the same task after `scheduled_at` passes
**When** workers poll eligible tasks
**Then** the task is claimed and executed normally
**And** execution ordering respects priority and schedule rules

**Given** an invalid or out-of-range schedule input
**When** submission is validated
**Then** a descriptive validation error is returned
**And** no task row is persisted for the invalid request

## Epic 2: Core Execution and Queue Processing

A developer can run durable multi-queue execution with atomic claiming, retries, and terminal failure handling.

### Story 2.1: Atomic Claiming and Concurrent Worker Safety

As a platform engineer,
I want workers to claim tasks atomically under concurrency,
So that no task is double-claimed and queue processing remains correct at scale.

**Acceptance Criteria:**

**Given** multiple workers polling the same queue concurrently
**When** pending tasks are available
**Then** each claimed task is locked and assigned to exactly one worker
**And** no duplicate claim occurs for the same task ID

**Given** queue ordering rules
**When** workers claim eligible tasks
**Then** claims prioritize higher priority first, then earlier `scheduled_at`
**And** tasks with future `scheduled_at` are excluded

**Given** no eligible tasks exist
**When** a claim cycle runs
**Then** the claim operation returns no task without error
**And** workers continue polling without entering a failure state

### Story 2.2: Retry, Backoff, and Terminal Failure Semantics

As an operator,
I want failed tasks to retry with controlled backoff and then enter a distinct terminal failure state,
So that transient issues recover automatically while exhausted tasks are clearly actionable.

**Acceptance Criteria:**

**Given** a task execution failure below `max_attempts`
**When** failure handling runs
**Then** the task is rescheduled with configured backoff
**And** attempts are incremented consistently

**Given** repeated failures reaching `max_attempts`
**When** the final allowed attempt fails
**Then** the task transitions to terminal `Failed` state (not re-queued)
**And** terminal-failure reason is persisted for diagnostics

**Given** mixed success and failure workload
**When** workers continue processing
**Then** successful tasks complete normally
**And** retrying and failed tasks do not block unrelated tasks in the same queue

### Story 2.3: Multi-Queue Concurrency Configuration

As a platform engineer,
I want independent queue-level concurrency behavior,
So that different workloads can be processed with appropriate throughput and isolation.

**Acceptance Criteria:**

**Given** at least two queues with different concurrency settings
**When** workers are started for both queues
**Then** each queue enforces its configured concurrency limit independently
**And** one queue's saturation does not prevent progress in the other queue

**Given** queue-specific workloads with different priorities
**When** processing runs under load
**Then** each queue preserves its own claim ordering semantics
**And** cross-queue interference does not violate per-queue guarantees

**Given** queue configuration validation
**When** invalid concurrency values are supplied
**Then** startup or config validation fails with clear error messaging
**And** no partially-initialized worker pool is left running

## Epic 3: Recovery and Runtime Resilience

An operator can trust service behavior through outages and shutdowns with lease safety and reconnect recovery.

### Story 3.1: Sweeper Zombie Recovery

As an operator,
I want expired running-task leases to be recovered automatically,
So that crashed or stalled workers do not leave tasks stuck indefinitely.

**Acceptance Criteria:**

**Given** tasks in `Running` with expired lease timestamps
**When** sweeper interval elapses
**Then** eligible tasks are moved back to executable state for retry
**And** recovery actions are emitted as observability signals

**Given** exhausted-attempt tasks with expired leases
**When** sweeper evaluates retry budget
**Then** tasks transition to terminal `Failed` state
**And** they are not recycled into pending execution

**Given** active healthy running tasks with valid leases
**When** sweeper runs
**Then** those tasks are not modified
**And** no false-positive recovery occurs

### Story 3.2: Graceful Shutdown and Lease Release

As an operator,
I want workers to drain or release leases during shutdown,
So that service restarts do not strand running tasks.

**Acceptance Criteria:**

**Given** a shutdown signal while workers are processing tasks
**When** graceful shutdown begins
**Then** in-flight tasks are allowed to finish within configured timeout
**And** worker intake of new tasks stops immediately

**Given** tasks still leased after shutdown timeout
**When** forced drain boundary is reached
**Then** remaining leases are released safely for future recovery
**And** no task is left permanently in orphaned `Running` state

**Given** shutdown orchestration across worker and sweeper components
**When** cancellation tokens propagate
**Then** subsystem termination follows documented order
**And** process exits with consistent final state

### Story 3.3: Postgres Reconnection and Pool Safety Limits

As an operator,
I want automatic reconnection behavior with enforced pool safety limits,
So that temporary database outages recover cleanly without uncontrolled resource usage.

**Acceptance Criteria:**

**Given** a temporary Postgres outage
**When** workers attempt claims and DB calls fail
**Then** the system retries with bounded backoff behavior
**And** processing resumes automatically after connectivity is restored

**Given** connection pool configuration exceeding documented ceiling
**When** engine startup and config validation run
**Then** initialization fails with explicit configuration error
**And** the process does not enter partial running state

**Given** pool saturation during high load
**When** workers contend for connections
**Then** saturation is surfaced through logs and metrics
**And** pending tasks are preserved without silent loss

## Epic 4: Operator Interfaces (REST and CLI)

Operators and external systems can submit, inspect, list, cancel, and discover API behavior through usable interfaces.

### Story 4.1: REST Task Submission and Status Lookup

As an external service integrator,
I want to submit tasks and retrieve task status via REST,
So that non-Rust systems can safely integrate with iron-defer execution flows.

**Acceptance Criteria:**

**Given** a valid task submission request
**When** `POST /tasks` is called
**Then** a task record is created with stable ID and initial state
**And** response schema matches documented API contract

**Given** an existing task ID
**When** `GET /tasks/{id}` is called
**Then** the current task state and relevant metadata are returned
**And** response fields follow documented serialization conventions

**Given** invalid payload or unknown task ID scenarios
**When** submit or lookup endpoints are called
**Then** API returns typed error responses with appropriate status codes
**And** no ambiguous or schema-breaking error body is emitted

### Story 4.2: REST Task Listing and Cancellation Controls

As an operator,
I want to list and cancel tasks through REST with clear filters and conflict behavior,
So that I can manage queue state safely during operations.

**Acceptance Criteria:**

**Given** task history across queues and statuses
**When** `GET /tasks` is called with queue and status filters
**Then** only matching tasks are returned with pagination metadata
**And** response ordering is stable and documented

**Given** a pending task
**When** `DELETE /tasks/{id}` is called
**Then** the task transitions to `Cancelled` and is no longer claimable
**And** cancellation result is reflected in subsequent lookups

**Given** already-running, terminal, or missing tasks
**When** cancellation is attempted
**Then** API returns deterministic conflict or not-found semantics
**And** no task-state corruption occurs under concurrent requests

### Story 4.3: CLI Operations for Submit, Inspect, and Validate

As an operator,
I want CLI commands for submit, inspect, worker status, and config validation,
So that I can operate the standalone service without direct database access.

**Acceptance Criteria:**

**Given** a running standalone deployment
**When** CLI submit and inspect commands are used
**Then** task creation and inspection workflows succeed through supported command flags
**And** outputs support human-readable defaults with optional machine-friendly formatting

**Given** active worker processes
**When** worker status command is executed
**Then** command reports worker and queue runtime visibility consistent with service state
**And** command exits with stable status codes

**Given** invalid or incomplete runtime configuration
**When** config validation command is executed before startup
**Then** actionable validation errors are displayed
**And** startup is blocked for invalid critical settings

## Epic 5: Observability and Health Visibility

Operators can monitor queue/system health and produce compliance-grade lifecycle evidence.

### Story 5.1: OTel Metrics, Failure Alerts, and Prometheus Scrape Endpoint

As an operator,
I want core execution and pool metrics exported with a Prometheus scrape endpoint,
So that I can monitor workload health and alert on terminal failures.

**Acceptance Criteria:**

**Given** normal task processing across queues
**When** metrics are recorded
**Then** queue depth, latency, retry/failure, and pool utilization instruments are emitted with stable names
**And** labels follow documented conventions

**Given** terminally failed tasks
**When** failure transitions are recorded
**Then** terminal-failure alert metric is emitted reliably
**And** operators can create alerts without parsing logs

**Given** a running standalone instance
**When** `GET /metrics` is called
**Then** response is valid Prometheus text exposition
**And** it includes current values for core engine metrics

### Story 5.2: Structured Lifecycle Logging and SQL-Queryable Transition Evidence

As a compliance-focused operator,
I want structured lifecycle logs and queryable transition records,
So that audits and incident investigations can reconstruct task behavior reliably.

**Acceptance Criteria:**

**Given** task lifecycle events (`Pending`, `Running`, terminal states)
**When** events are emitted
**Then** structured logs include `task_id`, `queue_name`, `worker_id`, and `attempt_number`
**And** payload content is excluded by default behavior

**Given** completed operational activity
**When** transition history is queried from the task store
**Then** state changes are retrievable with timestamps and worker identity
**And** query outputs are suitable for audit evidence workflows

**Given** log and task-store evidence
**When** a failed-task incident is investigated
**Then** operators can correlate lifecycle progression across both sources
**And** reconstructed timeline is complete enough for postmortem reporting

### Story 5.3: Queue Statistics, Health Probes, and OTel Compliance Test Evidence

As an operator,
I want queue stats and health probes with machine-verifiable OTel evidence tests,
So that runtime readiness and compliance posture are continuously verifiable.

**Acceptance Criteria:**

**Given** active and idle queues
**When** queue statistics endpoint is queried
**Then** queue depth and active processing visibility are returned consistently
**And** response behavior remains contract-stable for consumers

**Given** service and dependency health states
**When** liveness and readiness probes are queried
**Then** liveness reflects process health and readiness reflects Postgres connectivity
**And** degraded readiness returns clear non-200 semantics

**Given** compliance verification test suite
**When** OTel lifecycle evidence tests run
**Then** expected lifecycle signals are produced and assertable
**And** test outputs are suitable as machine-verifiable audit artifacts

## Epic 6: Deployment and Distribution

Teams can deploy, configure, and evaluate iron-defer in Docker/Kubernetes and example-driven workflows.

### Story 6.1: Docker Standalone Deployment Baseline

As an operator,
I want a working Docker image and compose flow for standalone iron-defer,
So that I can run the service in containerized environments quickly.

**Acceptance Criteria:**

**Given** the repository and documented compose artifacts
**When** Docker and Compose are used to build and run the standalone service
**Then** the service starts with required dependencies and reaches healthy state
**And** task submission and processing work end-to-end in the container setup

**Given** runtime configuration through environment variables
**When** the container starts with supplied env values
**Then** configuration is loaded according to documented precedence
**And** invalid critical settings fail startup with clear errors

**Given** image release expectations
**When** the runtime image is produced
**Then** the image includes required binaries and assets only
**And** operational docs are sufficient for local smoke execution

### Story 6.2: Kubernetes Deployment and Probe Integration

As a platform operator,
I want production-oriented Kubernetes manifests with probe wiring,
So that iron-defer can run reliably in orchestrated environments.

**Acceptance Criteria:**

**Given** provided Kubernetes manifests
**When** they are applied in a target cluster
**Then** deployment rolls out with expected config and service resources
**And** pods become ready only when dependency checks pass

**Given** liveness and readiness endpoint integration
**When** probe configuration is evaluated under healthy and degraded states
**Then** probe behavior matches documented runtime semantics
**And** restart and traffic decisions reflect actual service health

**Given** graceful termination settings
**When** pods receive termination signals
**Then** shutdown behavior honors configured grace windows
**And** no persistent orphaned running-task state is left behind

### Story 6.3: Feature Flags and Reference Example Validation

As a Rust developer,
I want feature flags and examples to be runnable and documented,
So that I can adopt iron-defer confidently across different runtime needs.

**Acceptance Criteria:**

**Given** supported Cargo feature flags
**When** builds are executed with default and selected feature combinations
**Then** enabled and disabled capabilities behave as documented
**And** invalid or conflicting combinations fail with clear guidance

**Given** repository reference examples
**When** example checks and runs are executed
**Then** examples compile and demonstrate documented workflows
**And** each example maps to a clear onboarding or operational scenario

**Given** docs and example cross-references
**When** developers follow the onboarding path
**Then** they can reach first successful task execution within documented expectations
**And** example instructions remain consistent with current API and configuration behavior

## Epic 7: Compliance and Privacy Baseline

Teams can operate with safe-by-default payload privacy and compliance guardrails in MVP behavior.

### Story 7.1: Privacy-Safe Logging Defaults and Opt-In Controls

As a security-conscious operator,
I want payload content excluded by default with explicit opt-in controls,
So that operational telemetry is safe by default while allowing controlled debugging.

**Acceptance Criteria:**

**Given** default runtime configuration
**When** tasks are processed and logs and metrics are emitted
**Then** payload content is excluded from emitted telemetry
**And** lifecycle correlation fields remain available for diagnostics

**Given** explicit payload-logging opt-in
**When** configuration enables payload visibility
**Then** payload inclusion behavior is applied only as documented
**And** opt-in state is clear and auditable in runtime configuration context

**Given** mixed environments (development and production)
**When** configuration profiles are validated
**Then** unsafe payload-logging combinations can be detected and flagged
**And** production guidance clearly discourages unsafe defaults

### Story 7.2: Queryable State-Transition Evidence for Compliance Workflows

As a compliance operator,
I want complete queryable transition evidence in the task store,
So that audits can verify lifecycle accountability without ad-hoc reconstruction.

**Acceptance Criteria:**

**Given** tasks moving through lifecycle states
**When** transitions are persisted
**Then** transition records include status progression, timestamps, and worker identity context
**And** records remain queryable using standard SQL patterns

**Given** audit evidence requests for specific task IDs
**When** operators run documented queries
**Then** full lifecycle timelines are reconstructable
**And** outputs satisfy compliance reporting expectations

**Given** high-volume transition history
**When** evidence queries execute
**Then** query behavior remains operationally usable
**And** retrieval semantics stay consistent across endpoints and storage views

### Story 7.3: UNLOGGED and Audit-Logging Mutual Exclusion Guardrail

As an operator,
I want startup validation to block unsafe `unlogged + audit_log` combinations,
So that durability and compliance contradictions cannot reach runtime.

**Acceptance Criteria:**

**Given** configuration enabling both UNLOGGED mode and audit logging
**When** engine startup or config validation runs
**Then** startup is rejected with explicit explanatory error
**And** no partial service initialization occurs

**Given** valid mutually compatible configurations
**When** startup validation executes
**Then** service boots normally without false-positive rejection
**And** selected mode behavior matches documentation

**Given** deployment automation and runbooks
**When** operators review and execute setup steps
**Then** the mutual-exclusion rule is clearly documented with rationale
**And** troubleshooting guidance exists for remediation

## Epic 8: Growth Tier 1 - Submission Safety

Developers get idempotent and transactional submission guarantees for adoption-critical correctness.

### Story 8.1: Idempotent Submission API and DB Uniqueness Enforcement

As a developer integrating retries,
I want duplicate submissions with the same idempotency key to return the original task,
So that retried client calls do not create duplicate work.

**Acceptance Criteria:**

**Given** submissions with identical `(queue, idempotency_key)` for active tasks
**When** duplicate requests are processed concurrently
**Then** exactly one task row is created
**And** duplicate requests return the existing task with stable HTTP semantics

**Given** terminal tasks associated with old idempotency keys
**When** retention window and cleanup rules apply
**Then** keys are eventually releasable per policy
**And** new valid submissions can reuse expired and released keys safely

**Given** idempotent and non-idempotent submissions mixed
**When** submission APIs run under load
**Then** idempotency behavior applies only when key is provided
**And** non-idempotent baseline behavior remains unchanged

### Story 8.2: Sweeper Idempotency-Key Retention Cleanup

As an operator,
I want expired idempotency keys cleaned through existing sweeper cycles,
So that key lifecycle is enforced without new background services.

**Acceptance Criteria:**

**Given** terminal tasks with retention-expired idempotency metadata
**When** sweeper tick executes cleanup logic
**Then** expired key retention data is removed according to policy
**And** cleanup does not affect active task idempotency protection

**Given** mixed sweeper responsibilities (zombie recovery and key cleanup)
**When** periodic sweeps run under normal load
**Then** both responsibilities execute without starvation
**And** observability signals expose cleanup outcomes

**Given** skewed clocks or near-boundary expiry cases
**When** cleanup evaluates expiration thresholds
**Then** behavior is deterministic and documented
**And** no premature key release occurs for still-protected tasks

### Story 8.3: Transactional Enqueue in Caller-Managed Transactions

As an application developer,
I want to enqueue tasks inside my existing database transaction,
So that task visibility is atomic with my business data commit and rollback.

**Acceptance Criteria:**

**Given** a caller-managed transaction that commits
**When** task enqueue is performed within that transaction
**Then** the task becomes visible only after commit
**And** worker processing does not occur before commit boundary

**Given** a caller-managed transaction that rolls back
**When** enqueue has been invoked in that transaction scope
**Then** no task remains visible after rollback
**And** no worker activation occurs for the rolled-back task

**Given** mixed transactional and non-transactional enqueue usage
**When** APIs are used in parallel workloads
**Then** each mode preserves its defined semantics independently
**And** transactional mode does not hold extra locks beyond the required insertion path

## Epic 9: Growth Tier 2 - Trace and Audit Evidence

Operators gain trace propagation and append-only audit evidence for compliance and diagnostics.

### Story 9.1: OTel Task Spans and W3C Trace Context Propagation

As an observability engineer,
I want task execution spans with propagated W3C trace context,
So that enqueue-to-execution tracing is continuous across distributed systems.

**Acceptance Criteria:**

**Given** task submissions with and without `traceparent` context
**When** tasks are claimed and executed
**Then** execution spans are created with required task attributes
**And** propagated context is applied when present without breaking backward compatibility

**Given** retries of the same task
**When** successive attempts execute
**Then** trace correlation preserves the originating trace identity
**And** retry attempts remain distinguishable in span attributes and events

**Given** configured telemetry export pipeline
**When** trace data is emitted
**Then** spans are exportable to the configured OTel backend
**And** failure paths degrade safely without blocking the task execution loop

### Story 9.2: OTel State-Transition Events and Export Verification

As a compliance operator,
I want state-transition events emitted as structured OTel signals with verified export behavior,
So that lifecycle evidence is machine-verifiable in observability backends.

**Acceptance Criteria:**

**Given** task transitions across lifecycle states
**When** transition hooks execute
**Then** OTel events are emitted with consistent structured attributes
**And** event vocabulary remains stable and documented

**Given** end-to-end telemetry configuration
**When** compliance tests execute
**Then** exporter path is verified beyond in-memory assertions
**And** emitted events are observable in configured collector and backend integration flow

**Given** invalid or unavailable exporter conditions
**When** event emission is attempted
**Then** failure handling is rate-safe and non-disruptive to task execution
**And** error signals are surfaced for operational diagnosis

### Story 9.3: Append-Only Audit Log with Atomic Transition Writes and Query API

As an auditor-facing operator,
I want immutable audit rows written atomically with task transitions and queryable by task ID,
So that compliance evidence is complete, tamper-evident, and operationally accessible.

**Acceptance Criteria:**

**Given** any task state transition
**When** transition persistence occurs
**Then** corresponding audit row is inserted in the same DB transaction
**And** no committed transition can exist without its audit record

**Given** append-only policy requirements
**When** update or delete operations are attempted against audit log rows
**Then** DB-level safeguards block mutation attempts
**And** immutability violations are surfaced clearly

**Given** an audit query for a task lifecycle
**When** operator retrieves records ordered by time
**Then** complete transition timeline is returned with trace linkage when available
**And** query shape is suitable for compliance and incident workflows

## Epic 10: Growth Tier 2 - Performance Mode

Operators can opt into UNLOGGED performance mode with explicit safety constraints.

### Story 10.1: Configurable UNLOGGED Table Mode and Conditional Schema Path

As an operator running high-throughput non-durable workloads,
I want to enable UNLOGGED task-table mode via configuration,
So that I can trade durability for significantly higher throughput in approved environments.

**Acceptance Criteria:**

**Given** `unlogged_tables=true` in valid configuration
**When** schema initialization runs for supported deployment mode
**Then** task table creation uses UNLOGGED semantics
**And** resulting schema reflects selected mode unambiguously

**Given** `unlogged_tables=false` (or omitted)
**When** initialization runs
**Then** standard logged table path is used
**And** behavior remains backward-compatible with baseline durability mode

**Given** mode switching requirements for existing deployments
**When** operator follows documented migration guidance
**Then** constraints and risks of switching are explicit
**And** misconfiguration risks are surfaced before runtime

### Story 10.2: UNLOGGED Throughput Benchmark and Safety Validation

As a performance-focused operator,
I want benchmark evidence and safety checks for UNLOGGED mode,
So that adoption decisions are data-driven and operationally safe.

**Acceptance Criteria:**

**Given** production-like benchmark configuration
**When** LOGGED and UNLOGGED modes are benchmarked on equivalent workload
**Then** throughput delta is measured and reported consistently
**And** evidence supports expected performance objective

**Given** audit and compliance-sensitive deployments
**When** operators evaluate mode suitability
**Then** documentation clearly states durability trade-offs and crash-truncation risk
**And** runbook guidance identifies approved usage contexts

**Given** monitoring and rollout planning
**When** UNLOGGED mode is enabled in non-durable environments
**Then** operational checks confirm expected behavior post-deploy
**And** rollback path to logged mode is documented and testable

## Epic 11: Growth Tier 3 - Checkpoint and Recovery Workflows

Developers can persist and resume multi-step workflow state safely across retries and crashes.

### Story 11.1: Checkpoint Persistence API in Task Context

As a workflow developer,
I want to persist checkpoint data during task execution,
So that partial progress survives worker failures.

**Acceptance Criteria:**

**Given** a running task with checkpoint-capable context
**When** handler invokes checkpoint persistence
**Then** checkpoint data is written atomically to task state
**And** write failures return typed errors to the handler path

**Given** repeated checkpoint updates in a single task lifecycle
**When** later checkpoint writes occur
**Then** latest checkpoint value is persisted as current recovery point
**And** checkpoint data remains bounded by documented payload constraints

**Given** task execution paths not using checkpoints
**When** handler completes normally
**Then** default behavior remains unchanged
**And** no extra persistence overhead is introduced beyond configured calls

### Story 11.2: Resume from Last Checkpoint on Retry

As a workflow developer,
I want retry attempts to load the last persisted checkpoint,
So that failed multi-step workflows resume instead of restarting from scratch.

**Acceptance Criteria:**

**Given** a task with previously persisted checkpoint data
**When** the task is retried after failure or recovery
**Then** the handler can access the last checkpoint via context API
**And** resumed execution can continue from that state deterministically

**Given** a task with no stored checkpoint
**When** retry logic executes
**Then** checkpoint lookup returns `None` or empty semantics consistently
**And** handler behavior remains predictable for first-run paths

**Given** checkpointed retries across multiple attempts
**When** each attempt updates and reads checkpoints
**Then** checkpoint progression remains coherent across attempts
**And** stale or partial checkpoint reads are prevented by persistence semantics

### Story 11.3: Checkpoint Lifecycle Cleanup on Completion

As an operator,
I want checkpoint data cleared when tasks complete successfully,
So that stored workflow state does not accumulate unnecessarily after terminal success.

**Acceptance Criteria:**

**Given** a task that used checkpoint persistence and reaches `Completed`
**When** completion transition is persisted
**Then** checkpoint data is cleared from task state
**And** subsequent lookups show no residual checkpoint payload

**Given** tasks ending in non-completed terminal states
**When** state transitions are finalized
**Then** checkpoint retention and cleanup behavior follows documented policy
**And** behavior is consistent across retry-exhausted and cancelled outcomes

**Given** mixed workloads with checkpoint and non-checkpoint tasks
**When** completion and cleanup paths execute under load
**Then** cleanup logic does not regress normal completion throughput
**And** observability can confirm checkpoint lifecycle behavior

## Epic 12: Growth Tier 3 - HITL and Data Residency

Teams can suspend/resume workflows and enforce geographic worker pinning for residency-sensitive workloads.

### Story 12.1: Suspend and Signal Resume Control Flow

As a workflow developer,
I want tasks to suspend and later resume via external signal,
So that human-in-the-loop approvals can pause execution without losing progress.

**Acceptance Criteria:**

**Given** a running task invoking suspend through context API
**When** suspend executes
**Then** the task transitions to `Suspended` with checkpoint-first behavior
**And** worker slot and claim resources are released

**Given** a suspended task
**When** `POST /tasks/{id}/signal` is called with optional payload
**Then** the task transitions back to executable state for the next claim cycle
**And** signal payload is available to resumed execution context

**Given** concurrent signal attempts for the same suspended task
**When** requests race
**Then** exactly one signal path succeeds
**And** losing requests receive deterministic conflict semantics

### Story 12.2: Suspended-State Concurrency and Timeout Watchdog

As an operator,
I want suspended tasks excluded from active concurrency and guarded by timeout policy,
So that paused workflows do not consume capacity indefinitely.

**Acceptance Criteria:**

**Given** tasks in `Suspended` state
**When** worker concurrency is evaluated
**Then** suspended tasks do not count toward active execution slots
**And** queue throughput for runnable tasks is unaffected by suspended count

**Given** suspended tasks exceeding configured timeout
**When** watchdog logic runs on sweeper cadence
**Then** overdue suspended tasks auto-transition to `Failed` with explicit reason
**And** timeout behavior is observable and auditable

**Given** zombie-recovery sweeps
**When** sweeper evaluates recovery candidates
**Then** suspended tasks are excluded from zombie recovery path
**And** suspend-timeout handling remains isolated to watchdog logic

### Story 12.3: Region-Labeled Submission and Region-Aware Claiming

As an operator in regulated environments,
I want region-labeled tasks to execute only on permitted workers,
So that data residency policies are enforced operationally.

**Acceptance Criteria:**

**Given** tasks submitted with specific region labels
**When** workers with mixed region assignments poll claims
**Then** only matching-region workers claim pinned tasks
**And** regionless workers do not claim region-pinned tasks

**Given** unpinned tasks without region labels
**When** workers poll across regions
**Then** unpinned tasks remain claimable per documented policy
**And** compatibility with existing non-regional workloads is preserved

**Given** queue stats and observability outputs
**When** region-aware workloads are active
**Then** region context is surfaced in supported stats and metric signals
**And** labeling policy avoids unbounded cardinality risk

## Epic 13: Release Cleanup and Readiness Gate

The team closes critical unresolved findings, enforces compatibility and observability safeguards, and gates release on auditable closure.

### Story 13.1: Close Critical Findings for Idempotency and Trace Export Paths (9.1, 10.1)

As a release manager,
I want unresolved critical findings in submission safety and trace export resolved,
So that release readiness is not blocked by correctness or observability gaps.

**Acceptance Criteria:**

**Given** open critical findings from stories 9.1 and 10.1
**When** remediation work is completed
**Then** actionable findings are fixed with evidence-linked changes
**And** any non-fixed findings are explicitly waived with written justification

**Given** OTLP export requirements for trace signals
**When** integration validation runs
**Then** exporter behavior is verified end-to-end (not only in-memory assertions)
**And** trace emission failures do not block task execution path

**Given** idempotency safety edge cases under concurrent retries
**When** tests execute against remediation changes
**Then** duplicate-creation and validation gaps are closed
**And** runtime and API behavior remains contract-consistent

### Story 13.2: Resolve Region and Queue-Contract Correctness Findings (12.2)

As an architect-operator,
I want unresolved region-pinning correctness and queue contract issues closed,
So that residency controls and API compatibility are safe for release.

**Acceptance Criteria:**

**Given** outstanding 12.2 correctness findings (compile, runtime, and contract)
**When** remediation is implemented
**Then** compile blockers and type mismatches are resolved
**And** regression coverage proves stable region-aware claim behavior

**Given** queue statistics contract requirements
**When** region-aware stats are exposed
**Then** existing endpoint behavior remains backward-compatible unless explicitly versioned or opt-in
**And** compatibility expectations are documented and tested

**Given** region-label observability safety requirements
**When** metrics are emitted for region-aware workloads
**Then** label cardinality is bounded or normalized per policy
**And** operational guidance documents safe labeling strategy

### Story 13.3: Stabilize E2E Quality Findings for Workflow Control and Residency Tests (12.3)

As a QA-focused maintainer,
I want unresolved E2E reliability findings closed,
So that release confidence is based on deterministic and maintainable test evidence.

**Acceptance Criteria:**

**Given** unresolved 12.3 E2E findings (deadlock risk, leak risk, non-deterministic assertions, helper gaps)
**When** remediation is applied
**Then** tests become deterministic and race-safe under CI conditions
**And** supporting helper infrastructure is completed and reused consistently

**Given** benchmark fairness and test signal quality concerns
**When** suite is executed post-remediation
**Then** benchmark and test setup follows documented fairness and isolation rules
**And** flaky or non-diagnostic assertions are replaced with robust checks

**Given** panic and error paths during test runs
**When** failures are induced
**Then** resources are released correctly and cleanup is verifiable
**And** failure output is actionable for rapid diagnosis

### Story 13.4: Tracking Reconciliation and Release Evidence Gate

As a release coordinator,
I want sprint/story tracking reconciled with closure evidence,
So that release decisions are based on auditable, current status.

**Acceptance Criteria:**

**Given** story and review states across affected growth stories
**When** closure verification is completed
**Then** `sprint-status.yaml` and story statuses are updated to reflect actual closure state
**And** no story is marked done while critical findings remain open

**Given** release-go/no-go evaluation
**When** evidence artifacts are reviewed
**Then** all critical findings are either fixed or explicitly waived with justification
**And** gate decision is documented with links to validating evidence

**Given** handoff between engineering, architecture, and QA roles
**When** release readiness is communicated
**Then** responsibilities and next actions are explicit
**And** readiness outcome is reproducible from stored artifacts
