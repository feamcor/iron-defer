---
stepsCompleted: ['step-01-detect-mode', 'step-02-load-context', 'step-03-risk-and-testability', 'step-04-coverage-plan', 'step-05-generate-output']
lastStep: 'step-05-generate-output'
lastSaved: '2026-04-04'
mode: 'system-level'
inputDocuments:
  - 'docs/artifacts/planning/prd.md'
  - 'docs/artifacts/planning/architecture.md'
  - 'docs/adr/0001-hexagonal-architecture.md'
  - 'docs/adr/0002-error-handling.md'
  - 'docs/adr/0003-configuration-management.md'
  - 'docs/adr/0004-async-runtime-tokio-ecosystem.md'
  - 'docs/adr/0005-database-layer-sqlx.md'
  - 'docs/adr/0006-serialization-serde.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/risk-governance.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/probability-impact.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/test-levels-framework.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/test-quality.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/adr-quality-readiness-checklist.md'
  - '_bmad/tea/workflows/testarch/bmad-testarch-test-design/resources/knowledge/test-priorities-matrix.md'
  - 'docs/guidelines/quality-gates.md'
---

## Step 1: Mode Detection

**Mode:** System-Level
**Reason:** PRD + ADRs + Architecture present; no sprint-status.yaml (epics not yet created)
**Prerequisites:** All satisfied

## Step 2: Load Context & Knowledge Base

### Configuration

| Setting | Value |
|---------|-------|
| `tea_use_playwright_utils` | `true` |
| `tea_use_pactjs_utils` | `false` |
| `tea_pact_mcp` | `none` |
| `tea_browser_automation` | `auto` |
| `test_stack_type` | `auto` → detected: **backend** (Cargo.toml, no frontend indicators) |
| `test_artifacts` | `docs/artifacts/test/` |

**Playwright profile:** API-only (backend stack, no `page.goto`/`page.locator` in test files)
**Contract testing:** Not loaded (`tea_use_pactjs_utils: false`, not relevant for embedded Rust library)

---

### Project Artifacts Loaded

#### Tech Stack & Dependencies

- **Language:** Rust 2024 edition, MSRV 1.94
- **Runtime:** Tokio (sole async runtime)
- **Database:** PostgreSQL 14+ via SQLx 0.8 (`runtime-tokio-rustls`, compile-time queries, embedded migrations)
- **HTTP:** axum 0.8 (server), reqwest 0.12 + rustls-tls (client)
- **TLS:** rustls only — OpenSSL banned via `deny.toml`
- **Observability:** opentelemetry 0.27, opentelemetry-otlp (OTLP/gRPC), tracing-subscriber JSON
- **Config:** figment + dotenvy + clap
- **Serialization:** serde + serde_json (camelCase API, snake_case DB)
- **Error handling:** thiserror (library crates), color-eyre (binary boundary)
- **Testing:** testcontainers 0.23, testcontainers-modules (postgres), tokio::test, cargo tarpaulin

#### Workspace Architecture

4-crate hexagonal workspace:
- `domain`: pure Rust types; no DB/IO; unsafe PROHIBITED; 80% coverage gate (tarpaulin)
- `application`: ports/adapters interface + services; mockall for unit tests
- `infrastructure`: PostgreSQL + OTel adapters; integration tests (testcontainers OnceCell shared DB per binary)
- `api`: axum REST + clap CLI + IronDefer builder; chaos tests (isolated containers per test)

#### Integration Points

1. **SKIP LOCKED atomic claiming** — single `UPDATE ... WHERE id=(SELECT ... FOR UPDATE SKIP LOCKED) RETURNING *`
2. **CancellationToken graceful shutdown** — tree-structured: root → worker_pool + sweeper + axum
3. **JoinSet + Semaphore worker pool** — bounded concurrency, drain on shutdown
4. **SweeperService** — separate tokio task, 60s interval, zombie recovery + max-attempts failure
5. **OTel OTLP/gRPC** — 7 metrics with `iron_defer_` prefix, structured JSON logs
6. **Dual deployment** — embedded lib (caller-provided PgPool) + standalone binary (owns runtime)
7. **sqlx offline mode** — `.sqlx/` committed, `SQLX_OFFLINE=true` in CI/Docker

#### NFRs Extracted

| NFR | Target | Verification |
|-----|--------|--------------|
| Throughput | ≥ 10,000 jobs/sec | `benches/throughput.rs` (criterion) |
| Task recovery | < lease duration | `chaos/worker_crash_test.rs` |
| Zero task loss | 100% in chaos tests | 4 chaos scenarios |
| At-least-once | Zero duplicates (normal conditions) | Integration assertions |
| Graceful shutdown | In-flight complete or leases released | `chaos/sigterm_test.rs` |
| Time-to-first-task | < 30 minutes | Developer onboarding |
| OTel coverage | Metrics + Logs (MVP) | Integration vs OTel Collector |
| PostgreSQL minimum | 14+ | CI matrix |
| Rust MSRV | 1.94+ | `cargo check` on MSRV toolchain |
| Domain coverage | 80% minimum (tarpaulin) | CI gate |
| Application coverage | 70% minimum | CI gate |

#### Compliance Frameworks (7)

PCI DSS v4.0.1 Req.10, GDPR Art.5/Ch.V, HIPAA Security Rule, DORA (EU 2022/2554), NIS2 Directive, SOC 2 CC7.2, ISO 27001:2022

**Coverage mechanism:** tasks table as audit trail, rustls TLS, structured logging with task_id, OTel metrics, cargo deny/audit, on-premises deployment, payload redaction (log_payload: false default)

---

### Knowledge Fragments Loaded (System-Level Required)

| Fragment | Purpose |
|----------|---------|
| `risk-governance.md` | Risk scoring (P×I), gate decisions, traceability matrix |
| `probability-impact.md` | 1-9 scale: DOCUMENT/MONITOR/MITIGATE/BLOCK thresholds |
| `test-levels-framework.md` | Unit/Integration/E2E decision matrix + anti-patterns |
| `test-quality.md` | Deterministic, isolated, explicit, focused, fast (<1.5min, <300 lines) |
| `adr-quality-readiness-checklist.md` | 8-category, 29-criteria NFR testability framework |
| `test-priorities-matrix.md` | P0-P3 classification, risk-based adjustments, coverage by priority |

---

### Pre-Analysis Summary (for Step 3)

**Highest-Risk Areas (candidates for P0 testing):**
1. SKIP LOCKED atomic claiming — correctness of at-least-once guarantee under concurrency
2. Sweeper zombie recovery — the primary resilience mechanism under worker failure
3. CancellationToken graceful shutdown — orphaned Running tasks on SIGTERM
4. Lease expiry / duplicate execution boundary — at-least-once vs exactly-once semantics
5. Max retries exhaustion → `failed` (never re-queued as `pending`)

**Testability Assets:**
- Port/adapter hexagonal model enables full unit testing of application layer with mocks (mockall)
- testcontainers shared OnceCell enables realistic integration tests without per-test DB spin-up
- Chaos test isolation (separate containers) enables destructive scenario testing
- `.sqlx/` offline cache enables build without live DB
- OTel collector testability via OTLP test receiver

**Known Coverage Gaps (from architecture):**
- No REST API authentication (MVP gap — documented, not tested)
- Exactly-once semantics not guaranteed (at-least-once only; idempotency is caller responsibility)
- LISTEN/NOTIFY deferred (Growth) — polling latency not tested
- W3C trace-context propagation deferred (Growth)

---

## Step 3: Testability & Risk Assessment

### Testability Review

#### Controllability

**✅ Strong:**
- Hexagonal port/adapter architecture: `TaskRepository` + `TaskExecutor` fully mockable via `mockall` + `#[automock]` — application layer tests need no database
- `testcontainers` OnceCell: controlled, deterministic DB state; migrations run once at init
- Chaos tests use isolated Postgres containers per test — zero inter-test contamination
- All key parameters configurable (`concurrency`, `poll_interval`, `sweeper_interval`, lease duration, `shutdown_timeout`) — timing controllable in tests
- `CancellationToken` injectable — shutdown sequencing testable without OS signals

**⚠️ Testability Concerns (Actionable):**

| ID | Concern | Severity | Action |
|----|---------|----------|--------|
| C1 | No dedicated test seeding API — test setup requires direct DB access via `PgPool` | Low | Accept: idiomatic for Rust; document pattern |
| C2 | SKIP LOCKED concurrent claiming tests are timing-dependent — determinism requires explicit multi-thread Tokio runtime control | Medium | **ACTIONABLE:** Specify `#[tokio::test(flavor="multi_thread")]` pattern in claiming tests |
| C3 | No named test OTel Collector harness — metric/log emission validation undefined | Medium | **ACTIONABLE:** Define mock OTLP receiver or stdout capture strategy for metrics tests |
| C6 | Sweeper 60s default interval too long for tests — tests need `WorkerConfig` with short interval or direct DB lease manipulation | Medium | **ACTIONABLE:** Specify test helper that sets sweeper_interval to ~1s and inserts expired leases |
| C7 | 30s shutdown drain timeout too long for tests — must configure short timeout or use `CancellationToken::cancel()` directly | Medium | **ACTIONABLE:** Specify `WorkerConfig::with_shutdown_timeout(Duration::from_secs(3))` pattern |

#### Observability

**✅ Strong:**
- `#[instrument(skip(self), fields(task_id, queue), err)]` on every public async method — all errors auto-tagged
- Structured JSON logs with `tracing-subscriber`; `env-filter` gives dynamic log level control
- 7 OTel metrics with defined names, types, labels — deterministic assertion targets
- JSON error responses with `SCREAMING_SNAKE_CASE` codes — stable assertion contract

**⚠️ Concerns:**
- **C4** — W3C trace context deferred to Growth: no distributed tracing across enqueue/dequeue boundary in MVP. Test scope: MVP spans are per-execution only.
- **C5** — OTel metrics test harness undefined: no named mechanism for asserting metric emission in integration tests (MONITOR).

#### Reliability

**✅ Strong:**
- Domain tests: zero external deps — fully deterministic and parallelizable
- Shared OnceCell pattern is parallel-safe across concurrent integration tests
- Chaos test isolation (independent containers) prevents cross-test contamination
- `log_payload: false` default prevents payload-dependent test flakiness

---

#### ADR Quality Readiness (29 Criteria)

| Category | Criteria Met | Status |
|----------|-------------|--------|
| 1. Testability & Automation | 2/4 | ⚠️ CONCERNS |
| 2. Test Data Strategy | 3/3 | ✅ PASS |
| 3. Scalability & Availability | 1/4 | ⚠️ CONCERNS |
| 4. Disaster Recovery | 0/3 | ⚠️ OUT OF MVP SCOPE |
| 5. Security | 2/4 | ⚠️ CONCERNS (no AuthN in MVP) |
| 6. Monitorability | 3/4 | ⚠️ CONCERNS (W3C trace deferred) |
| 7. QoS & QoE | 2/4 | ⚠️ CONCERNS (no rate limit, no P95 target) |
| 8. Deployability | 1/3 | ⚠️ CONCERNS (no zero-downtime, no auto-rollback) |
| **Total** | **14/29** | **⚠️ CONCERNS** |

Key testability-affecting gaps: C1 (state seeding), C3 (OTel harness), C5.1 (no AuthN — network isolation required), C6.1 (W3C trace deferred), 7.2 (no rate limiting on API).

---

### Risk Assessment Matrix

| ID | Risk | Category | P | I | Score | Action |
|----|------|----------|-|-|-------|--------|
| R005 | REST API unauthenticated — unauthorized task submission/cancellation | SEC | 3 | 3 | **9** | **BLOCK** |
| R001 | SKIP LOCKED claim query bug → duplicate task execution | TECH/DATA | 2 | 3 | **6** | MITIGATE |
| R002 | Sweeper fails to recover zombies (SQL bug or interval misconfiguration) | TECH/DATA | 2 | 3 | **6** | MITIGATE |
| R003 | Graceful shutdown orphans Running tasks (CancellationToken wiring) | TECH/OPS | 2 | 3 | **6** | MITIGATE |
| R004 | Parallel execution at lease expiry (slow worker + Sweeper simultaneous) | DATA | 3 | 2 | **6** | MITIGATE |
| R006 | SKIP LOCKED throughput degradation under high concurrency | PERF | 2 | 2 | 4 | MONITOR |
| R007 | `.sqlx/` cache stale → CI failure after query changes | TECH | 2 | 2 | 4 | MONITOR |
| R008 | Connection pool exhaustion in embedded mode | OPS | 2 | 2 | 4 | MONITOR |
| R009 | OTel Collector misconfiguration → signal loss | OPS | 2 | 2 | 4 | MONITOR |
| R011 | Misconfigured max_attempts → tasks fail permanently | BUS | 2 | 2 | 4 | MONITOR |
| R015 | Sweeper + slow original worker both execute same task | TECH/DATA | 2 | 2 | 4 | MONITOR |
| R010 | PII exposure via `log_payload: true` opt-in | SEC/DATA | 1 | 3 | 3 | DOCUMENT |
| R014 | Migration failure during upgrade → system non-functional | OPS | 1 | 3 | 3 | DOCUMENT |
| R012 | Supply chain CVE in transitive dependency | SEC | 1 | 2 | 2 | DOCUMENT |
| R013 | Large payload memory pressure via REST API | SEC | 1 | 2 | 2 | DOCUMENT |

### Risk Summary

**BLOCK (R005):** No authentication in MVP is an explicit, documented design decision — not a code defect. Mitigation is: deployment documentation + network isolation requirement + test that no escalation paths exist within the unauthenticated surface. Gate: documentation evidence of network boundary required.

**MITIGATE (R001, R002, R003, R004):** These four risks target the core correctness guarantee (at-least-once execution). All four have dedicated chaos test scenarios already planned in the architecture:
- R001 → `claiming_test.rs` (concurrent SKIP LOCKED correctness)
- R002 → `worker_crash_test.rs` (sweeper recovery)
- R003 → `sigterm_test.rs` (graceful drain)
- R004 → `claiming_test.rs` + `worker_crash_test.rs` (lease expiry boundary)

**MONITOR (6 risks):** Configuration validation, throughput benchmarks, and sqlx cache checks — covered by existing CI gates (`cargo sqlx prepare --check`, criterion benchmarks, OTel integration tests).

---

## Step 4: Coverage Plan & Execution Strategy

### Coverage Matrix

**Test Levels:** UNIT (domain pure functions) | INT (PostgreSQL via testcontainers) | API (axum HTTP round-trips) | CHAOS (isolated containers, failure injection) | BENCH (criterion throughput)

#### P0 — Critical (18 scenarios)

| ID | Scenario | Level | Risk | Crate |
|----|----------|-------|------|-------|
| P0-UNIT-001 | TaskStatus valid transitions (pending→running→completed/failed, pending→cancelled) | UNIT | R001 | domain |
| P0-UNIT-002 | TaskStatus invalid transitions rejected (completed→running, failed→pending) | UNIT | R001 | domain |
| P0-UNIT-003 | Retry formula: `min(base_delay * 2^(attempts-1), max_delay)` exponential + cap | UNIT | R002 | domain |
| P0-UNIT-004 | TaskId generation: valid UUID v4 | UNIT | — | domain |
| P0-INT-001 | Save task → find by id → all fields round-trip correctly | INT | — | infrastructure |
| P0-INT-002 | Single claim: pending→running, claimed_by set, attempts+1 | INT | R001 | infrastructure |
| P0-INT-003 | No pending tasks → claim returns None | INT | R001 | infrastructure |
| P0-INT-004 | Concurrent claims: N workers, 1 task → exactly 1 succeeds (SKIP LOCKED) | INT | R001 | infrastructure |
| P0-INT-005 | Scheduled_at in future → not claimed until due | INT | R001 | infrastructure |
| P0-INT-006 | Priority ordering: higher priority claimed first | INT | R001 | infrastructure |
| P0-INT-007 | Multi-queue isolation: queue A workers never claim queue B | INT | R001 | infrastructure |
| P0-INT-008 | Zombie: expired lease + attempts < max → sweeper recovers to pending | INT | R002 | infrastructure |
| P0-INT-009 | Zombie: expired lease + attempts >= max → sweeper sets failed | INT | R002 | infrastructure |
| P0-INT-010 | Valid lease (claimed_until > now) → sweeper ignores | INT | R002 | infrastructure |
| P0-CHAOS-001 | SIGTERM during execution → in-flight complete; zero orphaned Running | CHAOS | R003 | api |
| P0-CHAOS-002 | SIGTERM + drain timeout exceeded → leases released; exit | CHAOS | R003 | api |
| P0-API-001 | POST /tasks valid → 201 + pending in DB | API | — | api |
| P0-API-002 | POST /tasks invalid → 422 + INVALID_PAYLOAD | API | — | api |

#### P1 — High (23 scenarios)

| ID | Scenario | Level | Risk | Crate |
|----|----------|-------|------|-------|
| P1-INT-001 | Worker executes → completed, duration metric emitted | INT | — | infrastructure |
| P1-INT-002 | Handler error → retry/fail, last_error recorded | INT | R002 | infrastructure |
| P1-INT-003 | Semaphore: max N simultaneous executions | INT | — | application |
| P1-INT-004 | Poll interval: claims at configured cadence | INT | — | application |
| P1-INT-005 | Sweeper: `zombie_recoveries_total` counter on recovery | INT | — | infrastructure |
| P1-INT-006 | Sweeper interval: configurable, respected | INT | R002 | application |
| P1-UNIT-001 | Domain error types: variants, messages, Display | UNIT | — | domain |
| P1-UNIT-002 | Error From impls: context preserved across layers | UNIT | — | domain/app |
| P1-INT-007 | Postgres connection loss → worker retries next poll (no panic) | INT | R008 | infrastructure |
| P1-API-001 | GET /tasks/{id} → 200 + correct state | API | — | api |
| P1-API-002 | DELETE /tasks/{id} (pending) → cancelled | API | — | api |
| P1-API-003 | DELETE /tasks/{id} (running) → 409 Conflict | API | — | api |
| P1-API-004 | GET /tasks with filters → correct results | API | — | api |
| P1-API-005 | POST /tasks body "queue" → task in specified queue | API | — | api |
| P1-API-006 | GET /health → 200 | API | — | api |
| P1-API-007 | Body > 1 MiB → rejected | API | R013 | api |
| P1-UNIT-003 | TaskRegistry: dispatch by kind succeeds | UNIT | — | application |
| P1-UNIT-004 | TaskRegistry: unregistered kind → descriptive panic | UNIT | — | application |
| P1-UNIT-005 | Config layering precedence: defaults→file→.env→env→CLI | UNIT | — | api |
| P1-UNIT-006 | Missing DATABASE_URL → explicit error | UNIT | — | api |
| P1-INT-008 | IronDefer builder → migrations run, engine ready | INT | — | api |
| P1-INT-009 | engine.enqueue → task in DB as pending | INT | — | api |
| P1-CHAOS-001 | 100 tasks + kill worker → sweeper recovers all; 100 complete | CHAOS | R001/R002 | api |
| P1-CHAOS-002 | Postgres down → workers reconnect; no tasks lost | CHAOS | R008 | api |
| P1-CHAOS-003 | Max attempts exhausted → failed permanently; never re-queued | CHAOS | R002 | api |

#### P2 — Medium (14 scenarios)

| ID | Scenario | Level | Risk | Crate |
|----|----------|-------|------|-------|
| P2-INT-001 | `task_duration_seconds` histogram emitted | INT | C3/C5 | infrastructure |
| P2-INT-002 | Pending/running gauges match DB state | INT | C5 | infrastructure |
| P2-INT-003 | Worker pool utilization metric correct | INT | C5 | infrastructure |
| P2-INT-004 | Counter increments match actual attempts/failures | INT | C5 | infrastructure |
| P2-UNIT-001 | Default WorkerConfig: log_payload=false | UNIT | R010 | application |
| P2-INT-005 | log_payload=false → payload absent from tracing | INT | R010 | infrastructure |
| P2-INT-006 | log_payload=true → payload present in tracing | INT | R010 | infrastructure |
| P2-INT-007 | Fresh DB → migrations create tables + indexes | INT | R014 | infrastructure |
| P2-INT-008 | sqlx::migrate! path references workspace-root migrations/ | INT | R014 | infrastructure |
| P2-INT-009 | Pool exhaustion → workers block, recover when free | INT | R008 | infrastructure |
| P2-INT-010 | Pool metrics (available, in_use) emitted | INT | R008 | infrastructure |
| P2-API-001 | Error responses: no stack traces exposed | API | R005 | api |
| P2-API-002 | No hidden admin/debug endpoints | API | R005 | api |
| P2-INT-011 | IronDefer skip_migrations(true) → no migration run | INT | — | api |

#### P3 — Low (6 scenarios)

| ID | Scenario | Level | Risk | Crate |
|----|----------|-------|------|-------|
| P3-BENCH-001 | Throughput ≥ 10,000 jobs/sec (criterion) | BENCH | R006 | api |
| P3-BENCH-002 | Claim latency P95/P99 under high concurrency | BENCH | R006 | api |
| P3-INT-001 | CLI: submit task → task in DB | INT | — | api |
| P3-INT-002 | CLI: inspect queue → correct output | INT | — | api |
| P3-INT-003 | CLI: invalid config → clear error | INT | — | api |
| P3-INT-004 | Payload near 1 MiB limit → accepted | INT | R013 | api |

**Total: 61 scenarios** (18 P0 + 23 P1 + 14 P2 + 6 P3)

---

### Execution Strategy

| Cadence | Suite | Duration | Contents |
|---------|-------|----------|----------|
| **PR** | `cargo test --workspace` + CI gates | ~8-12 min | All UNIT + INT + API tests. fmt, clippy, deny, audit, machete, sqlx check, tarpaulin (domain). |
| **Nightly** | Chaos + full coverage | ~15-25 min | 4 CHAOS tests (isolated containers). Full tarpaulin report. OTel integration. |
| **Weekly/Release** | Benchmarks + deploy | ~10-15 min | Criterion throughput (external DB). Docker build. K8s dry-run. CLI tests. |

---

### Resource Estimates

| Priority | Scenarios | Effort | Notes |
|----------|-----------|--------|-------|
| P0 | 18 | ~25–40 hrs | Concurrent claiming, SKIP LOCKED correctness, chaos shutdown — most complex |
| P1 | 23 | ~20–35 hrs | Worker pool, REST API, config, crash chaos — builds on P0 infrastructure |
| P2 | 14 | ~10–20 hrs | OTel metrics (mock collector TBD), payload privacy, migrations |
| P3 | 6 | ~3–8 hrs | Benchmarks (external DB), CLI wrappers |
| **Total** | **61** | **~58–103 hrs** | Lower bound if test infra is clean; upper if OTel mock + race testing need iteration |

---

### Quality Gates

| Gate | Threshold | Enforcement |
|------|-----------|-------------|
| P0 pass rate | 100% | PR blocks on any P0 failure |
| P1 pass rate | ≥ 95% | PR blocks; max 1 known-failing with ticket |
| Domain coverage | ≥ 80% | `cargo tarpaulin -p iron-defer-domain --fail-under 80` (CI) |
| Application coverage | ≥ 70% | `cargo tarpaulin -p iron-defer-application --fail-under 70` (CI) |
| Chaos tests | 4/4 pass | Nightly: SIGTERM, worker crash, DB outage, max retries |
| R005 (BLOCK) | Documented | Network isolation in deployment guide + README |
| R001-R004 (MITIGATE) | Named tests pass | Each risk has dedicated coverage in matrix |
| Clippy | Zero warnings | `cargo clippy --workspace -- -D clippy::pedantic` (PR) |
| Dependencies | Zero violations | `cargo deny check` + `cargo audit` + `cargo machete` (PR) |
| sqlx cache | Current | `cargo sqlx prepare --check --workspace` (PR) |

---

## Step 5: Generate Outputs & Validate

### Execution Mode

**Resolved mode:** Sequential (no agent-team or subagent runtime available)

### Output File Paths

| Document | Path |
|----------|------|
| Architecture Test Design | `docs/artifacts/test/test-design-architecture.md` |
| QA Test Design | `docs/artifacts/test/test-design-qa.md` |
| BMAD Handoff | `docs/artifacts/test/test-design/iron-defer-handoff.md` |
| Progress Log | `docs/artifacts/test/test-design-progress.md` |

### Key Risks and Gate Thresholds

- **1 BLOCK risk (R005):** Unauthenticated REST API — mitigated by deployment documentation + network isolation
- **4 MITIGATE risks (R001-R004):** Core at-least-once correctness — mitigated by 18 P0 test scenarios + 5 chaos tests
- **Quality gates:** P0 100%, P1 >= 95%, domain coverage >= 80%, all 4 chaos tests pass nightly

### Open Assumptions

1. Docker available in CI for testcontainers (GitHub Actions services as fallback)
2. OTel test harness strategy (C3) to be decided during implementation — blocks P2 metrics tests
3. Criterion benchmarks require external DATABASE_URL — separate CI job (release.yml, not ci.yml)

### Validation Checklist Summary

- All prerequisites met (PRD, architecture, ADRs present)
- All 5 process steps completed (detect mode, load context, risk assessment, coverage plan, generate output)
- Risk matrix: 15 risks with unique IDs, correct P x I scores, mitigation for all score >= 6
- Coverage matrix: 61 scenarios, no duplicate coverage across levels, priorities P0-P3 assigned
- Execution strategy: PR / Nightly / Weekly model, PR < 15 min target
- Resource estimates: interval ranges (~58-103 hours total)
- Quality gates defined with thresholds
- Architecture doc: actionable-first structure, ~180 lines, no test implementation code
- QA doc: test execution recipe with full scenario tables, code examples in Rust
- Handoff doc: risk-to-story mapping, epic-level gates, phase transition criteria
- Cross-document consistency: same risk IDs, same priorities, same terminology
