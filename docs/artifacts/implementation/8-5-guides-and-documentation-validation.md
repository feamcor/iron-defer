# Story 8.5: Guides & Documentation Validation

Status: done

## Story

As a developer,
I want comprehensive guides for every deployment and integration mode,
so that I can configure iron-defer correctly for my specific use case with confidence that the documentation is accurate.

## Acceptance Criteria

### AC1: User-Facing Guides

Given the `docs/` directory,
When I look for user-facing guides,
Then the following guides exist:
- `docs/guides/embedded-library.md` — embedding iron-defer in an existing Tokio app (builder API, caller-provided pool, migration opt-out)
- `docs/guides/standalone-binary.md` — running the standalone binary (Docker, env vars, CLI subcommands)
- `docs/guides/rest-api.md` — complete REST API reference (all endpoints, request/response examples, error codes)
- `docs/guides/deployment.md` — Docker Compose and Kubernetes deployment (references to manifests, env var configuration, graceful shutdown)
- `docs/guides/observability.md` — OTel metrics, structured logging, Prometheus scraping, payload privacy
- `docs/guides/configuration.md` — complete configuration reference (figment chain, all config fields, env var naming, defaults)

### AC2: Accurate Cross-References

Given each guide,
When it references a code example or API endpoint,
Then the reference points to an actual example file in `crates/api/examples/` or an actual endpoint in the codebase,
And no guide references features, endpoints, or config fields that don't exist.

### AC3: E2E-Validated Documentation Chain

Given the E2E-validated documentation principle (CR57),
When the documentation suite is complete,
Then each guide's "getting started" path is covered by at least one E2E or integration test,
And the README links to guides, guides reference examples, examples are compiled by CI (`cargo check --examples`),
And a documentation map exists (in README or `docs/index.md`) showing the relationship: README → guide → example → test.

### AC4: No Documentation Drift

Given all documentation,
When `cargo test --workspace` runs,
Then no test failures indicate documentation drift (examples compile, referenced endpoints exist).

## Tasks / Subtasks

- [x] **Task 1: Create `docs/guides/` directory and embedded-library guide** (AC: 1, 2)
  - [x] 1.1: Created `docs/guides/embedded-library.md`
  - [x] 1.2: Documented full builder method table (12 methods)
  - [x] 1.3: Documented caller-provided pool pattern
  - [x] 1.4: Documented migration opt-out with `skip_migrations(true)`
  - [x] 1.5: Documented `Task` trait implementation with code example (note: users implement `Task`, not `TaskHandler`)
  - [x] 1.6: Documented `engine.start(token)` for starting workers (note: `start_workers` does not exist)
  - [x] 1.7: Referenced `basic_enqueue.rs`, `axum_integration.rs`, and `multi_queue.rs`
  - [x] 1.8: All referenced methods verified in codebase

- [x] **Task 2: Create standalone-binary guide** (AC: 1, 2)
  - [x] 2.1: Created `docs/guides/standalone-binary.md`
  - [x] 2.2-2.7: Documented all CLI subcommands and global flags
  - [x] 2.8-2.9: Documented Docker usage with references to Dockerfile and compose files

- [x] **Task 3: Create REST API reference guide** (AC: 1, 2)
  - [x] 3.1: Created `docs/guides/rest-api.md`
  - [x] 3.2: All 9 endpoints documented with method, path, description
  - [x] 3.3: Request/response JSON examples with camelCase fields
  - [x] 3.4: Error response format documented
  - [x] 3.5: All 7 error codes documented with HTTP status
  - [x] 3.6: Referenced live OpenAPI spec at `/openapi.json`

- [x] **Task 4: Create deployment guide** (AC: 1, 2)
  - [x] 4.1: Created `docs/guides/deployment.md`
  - [x] 4.2: Docker Compose reference to `docker/docker-compose.yml`
  - [x] 4.3: Docker build reference to `docker/Dockerfile` (no k8s manifests referenced)
  - [x] 4.4: Environment variable configuration with `IRON_DEFER__` prefix
  - [x] 4.5: Graceful shutdown behavior documented
  - [x] 4.6: Inline K8s probe YAML examples (no non-existent files referenced)
  - [x] 4.7: Referenced `docker/smoke-test.sh`
  - [x] 4.8: Referenced `docker/docker-compose.dev.yml`

- [x] **Task 5: Create observability guide** (AC: 1, 2)
  - [x] 5.1: Created `docs/guides/observability.md`
  - [x] 5.2: All 7 OTel metrics documented with types and labels
  - [x] 5.3: Prometheus scraping documented
  - [x] 5.4: Structured logging with reference to guidelines
  - [x] 5.5: Payload privacy documented
  - [x] 5.6: OTLP export configuration documented
  - [x] 5.7: Referenced compliance-evidence.md

- [x] **Task 6: Create configuration reference guide** (AC: 1, 2)
  - [x] 6.1: Created `docs/guides/configuration.md`
  - [x] 6.2: Figment 6-step precedence chain documented
  - [x] 6.3: All AppConfig fields with types, defaults, env vars in tables
  - [x] 6.4: `IRON_DEFER_PROFILE` documented
  - [x] 6.5: `IRON_DEFER__` prefix nesting documented
  - [x] 6.6: Example `config.toml` provided
  - [x] 6.7: Referenced config.rs as authoritative source

- [x] **Task 7: Create documentation map** (AC: 3)
  - [x] 7.1: Created `docs/index.md` with full documentation structure
  - [x] 7.2: README → guides → examples → tests chain mapped
  - [x] 7.3: Each guide mapped to validating E2E/integration test
  - [x] 7.4: README updated with link to `docs/index.md` and all 6 guide links

- [x] **Task 8: Cross-reference validation** (AC: 2, 4)
  - [x] 8.1: All file paths in guides verified (11/11 exist)
  - [x] 8.2: All method/function references verified against source
  - [x] 8.3: All endpoint paths verified against router.rs
  - [x] 8.4: `cargo check --examples` passes (4/4 examples)
  - [x] 8.5: `cargo test --workspace --no-run` passes (no compilation failures)

## Dev Notes

### Current Documentation State

**Existing docs structure:**
```
docs/
├── adr/                              # 6 architecture decision records
│   ├── 0001-hexagonal-architecture.md
│   ├── 0002-error-handling.md
│   ├── 0003-configuration-management.md
│   ├── 0004-async-runtime-tokio-ecosystem.md
│   ├── 0005-database-layer-sqlx.md
│   └── 0006-serialization-serde.md
└── guidelines/                       # 5 guideline documents
    ├── compliance-evidence.md
    ├── postgres-reconnection.md
    ├── quality-gates.md
    ├── rust-idioms.md
    ├── security.md
    └── structured-logging.md
```

**No `docs/guides/` directory exists** — all 6 guides are new files.

### Configuration Reference Source

From `crates/api/src/config.rs`, the `AppConfig` struct and its sub-structs define all configuration fields. The figment chain precedence (lines 1-9):
1. Compiled defaults (`AppConfig::default()`)
2. Base config file (`config.toml`)
3. Profile overlay (`config.{IRON_DEFER_PROFILE}.toml`)
4. `.env` file (loaded by dotenvy)
5. Environment variables (`IRON_DEFER__` prefix)
6. CLI flags (always win)

Read the full `config.rs` to extract all `AppConfig` fields, their types, and default values for Task 6.

### REST API Endpoint Reference

From `crates/api/src/http/router.rs`, all registered routes:
- `POST /tasks` — `tasks::create_task`
- `GET /tasks` — `tasks::list_tasks`
- `GET /tasks/{id}` — `tasks::get_task`
- `DELETE /tasks/{id}` — `tasks::cancel_task`
- `GET /queues` — `queues::list_queues`
- `GET /health` — `health::liveness`
- `GET /health/ready` — `health::readiness`
- `GET /metrics` — `metrics::scrape`
- `GET /openapi.json` — OpenAPI spec

Request/response types are in `crates/api/src/http/handlers/tasks.rs`:
- `CreateTaskRequest`: queue (optional, default "default"), kind, payload, scheduledAt, priority, maxAttempts — all camelCase
- `TaskResponse`: id, queue, kind, payload, status, priority, attempts, maxAttempts, lastError, scheduledAt, claimedBy, claimedUntil, createdAt, updatedAt — all camelCase
- List params: limit (default 50 with filters, 100 without filters — `MAX_LIST_LIMIT`), offset (max 10,000)

### CLI Command Reference

From `crates/api/src/cli/mod.rs`, subcommands:
1. `serve` (default) — `--port`, `--concurrency`, `--otlp-endpoint`
2. `submit` — `--queue`, `--kind`, `--payload`, `--scheduled-at`, `--priority`, `--max-attempts`
3. `tasks` — `--queue`, `--status`
4. `workers` — show active workers
5. `config validate` — config validation (note: `config` alone requires a sub-subcommand)

Global flags: `--config/-c` (or `IRON_DEFER_CONFIG`), `--database-url` (or `DATABASE_URL`), `--json`

### Docker Manifests (Verified)

Confirmed existing files:
- `docker/Dockerfile` — multi-stage build
- `docker/docker-compose.yml` — local dev/production setup
- `docker/docker-compose.dev.yml` — development setup
- `docker/smoke-test.sh` — deployment validation script

**NOT present:** `docker/k8s/` directory does NOT exist. Do not reference Kubernetes manifests as existing files. If the deployment guide includes K8s guidance, provide generic inline YAML examples based on the Docker image.

### Observability Metrics

From `crates/application/src/metrics.rs`, metric instrument names:
- Verify exact names by reading the file — expected prefix: `iron_defer_`
- Gauges, counters, histograms with `queue`, `kind`, `status` labels
- Prometheus scrape at `GET /metrics` via `prometheus::TextEncoder`

From `crates/api/src/http/handlers/metrics.rs`:
- Returns 404 if Prometheus registry not configured
- Content-Type: `text/plain; version=0.0.4; charset=utf-8`

### Guide Length and Structure

Each guide should be 80-200 lines, following this structure:
```
# Guide Title
## Overview (2-3 sentences)
## Prerequisites
## Step-by-step instructions
## Configuration options (if applicable)
## Examples (reference crates/api/examples/)
## Troubleshooting (optional, only if common pitfalls exist)
```

### Anti-Patterns to Avoid

- **Do NOT copy-paste entire source files into guides** — reference file paths, show minimal code snippets (3-10 lines)
- **Do NOT document aspirational features** — only document what exists in the current codebase
- **Do NOT create separate guide files for topics that are already covered in guidelines/** — the existing `docs/guidelines/` files cover internal conventions; `docs/guides/` covers user-facing how-tos
- **Do NOT duplicate ADR content** — reference ADRs for decision rationale, keep guides focused on "how to"
- **Do NOT invent config fields** — read `AppConfig` struct from source for authoritative field list
- **Do NOT create a config.toml example with fields that don't exist** — verify every field against `config.rs`
- **Do NOT hardcode specific versions** in guides — use "current" or reference `Cargo.toml` for version info

### Dependency on Story 8.4

This story depends on Story 8.4 for:
- The rewritten README.md (Task 7 adds documentation map link)
- The new examples (`retry_and_backoff.rs`, `multi_queue.rs`) that guides should reference

If 8.4 is not yet done, document references to the expected example files and note them for cross-validation.

### Previous Story Intelligence

**From Story 8.1 (done):**
- Architecture document fully reconciled — all file paths, API names, and patterns are authoritative
- Engineering standards section added — covers newtype, builder, typestate, trait, accessor patterns
- 25+ source file comments use stable section-name references

**From Epic 7 retrospective:**
- Documentation depth mandate: progressive complexity, every snippet executable
- README → guide → example → test chain must be traceable (CR57)

**From Epic 6 (deferred items):**
- All 14 deferred items resolved — no pending work that could affect documentation accuracy

### Project Structure Notes

New files to create:
- `docs/guides/embedded-library.md`
- `docs/guides/standalone-binary.md`
- `docs/guides/rest-api.md`
- `docs/guides/deployment.md`
- `docs/guides/observability.md`
- `docs/guides/configuration.md`
- `docs/index.md` — documentation map

Files to update:
- `README.md` — add link to `docs/index.md` (coordinate with Story 8.4)

### References

- [Source: docs/artifacts/planning/epics.md, Lines 861-899 — Story 8.5 definition, CR56+CR57]
- [Source: crates/api/src/http/router.rs — all HTTP endpoint registrations]
- [Source: crates/api/src/http/handlers/tasks.rs — CreateTaskRequest, TaskResponse structs]
- [Source: crates/api/src/http/handlers/health.rs — liveness and readiness handlers]
- [Source: crates/api/src/http/handlers/queues.rs — QueueStatsResponse]
- [Source: crates/api/src/http/handlers/metrics.rs — Prometheus encoder, 404 behavior]
- [Source: crates/api/src/cli/mod.rs — CLI subcommands and global flags]
- [Source: crates/api/src/cli/submit.rs — Submit command flags]
- [Source: crates/api/src/config.rs — AppConfig, figment chain precedence]
- [Source: crates/application/src/metrics.rs — OTel metric instrument definitions]
- [Source: crates/application/src/registry.rs — TaskHandler trait]
- [Source: crates/api/src/lib.rs — IronDefer builder API]
- [Source: docker/Dockerfile — multi-stage build]
- [Source: docker/docker-compose.yml — local dev setup]
- [NOTE: docker/k8s/ does NOT exist — no Kubernetes manifests in repo]
- [Source: docker/smoke-test.sh — deployment validation]
- [Source: docs/guidelines/structured-logging.md — logging field glossary]
- [Source: docs/guidelines/compliance-evidence.md — audit query reference]
- [Source: docs/adr/0003-configuration-management.md — config design rationale]
- [Source: docs/artifacts/planning/architecture.md §Configuration Chain — figment 6-step precedence]
- [Source: docs/artifacts/implementation/8-1-architecture-reconciliation-and-engineering-standards.md — previous story]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Story spec referenced `start_workers(queue, concurrency)` which doesn't exist — documented `engine.start(token)` instead
- Story spec referenced `TaskHandler` trait for user implementation — corrected to `Task` trait (TaskHandler is internal)
- No k8s manifests exist in repo — provided inline YAML examples instead of file references

### Completion Notes List

- 6 user-facing guides created in `docs/guides/`: embedded-library, standalone-binary, rest-api, deployment, observability, configuration
- Documentation map created at `docs/index.md` mapping README → guides → examples → tests
- README updated with links to all 6 guides and documentation map
- All cross-references validated: 11/11 file paths exist, all methods/endpoints verified
- `cargo check --examples` passes, `cargo test --workspace --no-run` passes

### Change Log

- 2026-04-24: Implemented all 8 tasks for Story 8.5 — 6 guides, documentation map, README links, cross-reference validation

### File List

- docs/guides/embedded-library.md (new — builder API, pool, migrations, Task trait)
- docs/guides/standalone-binary.md (new — CLI subcommands, global flags, Docker)
- docs/guides/rest-api.md (new — all endpoints, request/response examples, error codes)
- docs/guides/deployment.md (new — Docker Compose, Docker build, env vars, K8s probes, shutdown)
- docs/guides/observability.md (new — 7 metrics, Prometheus, logging, OTLP, privacy)
- docs/guides/configuration.md (new — figment chain, all config fields, example config.toml)
- docs/index.md (new — documentation map: README → guides → examples → tests)
- README.md (modified — added guides section and docs/index.md link)
