---
stepsCompleted: ['step-05-generate-output']
lastStep: 'step-05-generate-output'
lastSaved: '2026-04-04'
workflowType: 'testarch-test-design'
inputDocuments:
  - 'docs/artifacts/planning/prd.md'
  - 'docs/artifacts/planning/architecture.md'
  - 'docs/adr/0001-hexagonal-architecture.md'
  - 'docs/adr/0002-error-handling.md'
  - 'docs/adr/0003-configuration-management.md'
  - 'docs/adr/0004-async-runtime-tokio-ecosystem.md'
  - 'docs/adr/0005-database-layer-sqlx.md'
  - 'docs/adr/0006-serialization-serde.md'
---

# Test Design for QA: iron-defer

**Purpose:** Test execution recipe. Defines what to test, how to test it, and what is needed from other teams.

**Date:** 2026-04-04
**Author:** Fabio (via TEA)
**Status:** Draft
**Project:** iron-defer

**Related:** See Architecture doc (`test-design-architecture.md`) for testability concerns and architectural blockers.

---

## Executive Summary

**Scope:** Complete MVP test coverage for iron-defer: task lifecycle, SKIP LOCKED claiming, worker pool, sweeper recovery, graceful shutdown, REST API, CLI, OTel observability, dual deployment.

**Risk Summary:**

- Total Risks: 15 (5 high-priority >= 6, 6 medium, 4 low)
- Critical Categories: TECH/DATA (claiming correctness, zombie recovery), SEC (unauthenticated API)

**Coverage Summary:**

- P0 tests: 18 (SKIP LOCKED, sweeper, shutdown, REST core)
- P1 tests: 23 (worker pool, error handling, REST extended, config, chaos)
- P2 tests: 14 (OTel metrics, payload privacy, migrations, pool)
- P3 tests: 6 (benchmarks, CLI, edge cases)
- **Total**: 61 tests (~3-5 weeks with 1 developer)

---

## Not in Scope

| Item | Reasoning | Mitigation |
|------|-----------|------------|
| **REST API authentication** | MVP has no auth; Growth phase addition | Network isolation documented in deployment guide (R005) |
| **W3C trace context** | Deferred to Growth phase | Per-execution spans tested; cross-boundary tracing not tested |
| **LISTEN/NOTIFY** | Deferred to Growth; polling (500ms) in MVP | Poll interval tests cover MVP behavior |
| **Multi-backend support** | MVP targets PostgreSQL only | All tests use Postgres via testcontainers |
| **UI/dashboard testing** | No UI in MVP | REST API and CLI tested directly |

---

## Dependencies & Test Blockers

**CRITICAL:** Testing cannot proceed without these items.

### Dev Dependencies (Pre-Implementation)

**Source:** See Architecture doc "Quick Guide" for detailed mitigation plans.

1. **OTel test harness (C3)** - Dev - Week 2
   - Need mock OTLP receiver or stdout capture for metric/log assertions
   - Blocks P2 OTel tests (P2-INT-001 through P2-INT-004)

2. **Sweeper test timing (C6)** - Dev - Week 3
   - Need `WorkerConfig` with ~1s sweeper_interval and DB lease injection helper
   - Blocks P0 sweeper tests (P0-INT-008/009/010) and chaos tests

3. **Shutdown test timing (C7)** - Dev - Week 3
   - Need configurable short shutdown_timeout (3s) for test suites
   - Blocks P0 shutdown chaos tests (P0-CHAOS-001/002)

### Test Infrastructure Setup

1. **testcontainers shared DB** - Dev
   - `OnceCell<(PgPool, ContainerAsync<Postgres>)>` in `crates/infrastructure/tests/common/mod.rs`
   - Migrations run once via `sqlx::migrate!("../../migrations")`
   - All integration tests call `test_pool()`, never spin up own container

2. **Chaos test isolation** - Dev
   - Each test in `crates/api/tests/chaos/` spins up its own Postgres container
   - Must NOT use shared `TEST_DB OnceCell`

**Example test pattern (Rust):**

```rust
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

#[tokio::test]
async fn task_save_and_find_roundtrip() {
    let pool = test_pool().await;
    // ... test logic with explicit assertions
}
```

---

## Risk Assessment

**Note:** Full risk details in Architecture doc. This section shows QA test coverage per risk.

### High-Priority Risks (Score >= 6)

| Risk ID | Category | Description | Score | QA Test Coverage |
|---------|----------|-------------|-------|------------------|
| **R005** | SEC | Unauthenticated REST API | **9** | P2-API-001 (no stack traces), P2-API-002 (no hidden endpoints) |
| **R001** | TECH/DATA | SKIP LOCKED claim bug → duplicates | **6** | P0-INT-002/003/004/005/006/007 (6 claiming scenarios) |
| **R002** | TECH/DATA | Sweeper fails zombie recovery | **6** | P0-INT-008/009/010 + P1-CHAOS-001 (4 scenarios) |
| **R003** | TECH/OPS | Shutdown orphans Running tasks | **6** | P0-CHAOS-001/002 (2 chaos scenarios) |
| **R004** | DATA | Parallel execution at lease expiry | **6** | P0-INT-010 (valid lease ignored) + P1-INT-005 (recovery counter) |

### Medium/Low-Priority Risks

| Risk ID | Category | Description | Score | QA Test Coverage |
|---------|----------|-------------|-------|------------------|
| R006 | PERF | Throughput degradation | 4 | P3-BENCH-001/002 (criterion benchmarks) |
| R007 | TECH | sqlx cache stale | 4 | `cargo sqlx prepare --check` in CI |
| R008 | OPS | Pool exhaustion | 4 | P1-INT-007 (connection loss), P2-INT-009/010 (pool metrics) |
| R009 | OPS | OTel signal loss | 4 | P2-INT-001-004 (metric emission tests) |
| R010 | SEC/DATA | PII via log_payload | 3 | P2-UNIT-001, P2-INT-005/006 (payload privacy) |
| R011 | BUS | max_attempts misconfiguration | 4 | P1-CHAOS-003 (max retries exhausted) |

---

## Entry Criteria

- [ ] Cargo workspace compiles (`cargo check --workspace`)
- [ ] PostgreSQL migrations apply successfully
- [ ] testcontainers Docker access available (CI and local)
- [ ] Pre-implementation blockers resolved (C3, C6, C7)
- [ ] CI pipeline configured (fmt, clippy, deny, audit, machete, sqlx check)

## Exit Criteria

- [ ] All P0 tests passing (100%)
- [ ] All P1 tests passing (>= 95%)
- [ ] No open high-severity bugs in SKIP LOCKED claiming or sweeper recovery
- [ ] Domain crate coverage >= 80% (tarpaulin)
- [ ] All 4 chaos scenarios pass on nightly
- [ ] R005 documentation evidence complete

---

## Test Coverage Plan

**IMPORTANT:** P0/P1/P2/P3 = **priority and risk level**, NOT execution timing. See "Execution Strategy" for when tests run.

### P0 (Critical)

**Criteria:** Blocks core at-least-once guarantee + High risk (>= 6) + No workaround

| Test ID | Requirement | Test Level | Risk Link | Notes |
|---------|-------------|------------|-----------|-------|
| **P0-UNIT-001** | TaskStatus valid transitions | UNIT | R001 | pending->running->completed/failed, pending->cancelled |
| **P0-UNIT-002** | TaskStatus invalid transitions rejected | UNIT | R001 | completed->running, failed->pending must error |
| **P0-UNIT-003** | Retry formula exponential + cap | UNIT | R002 | `min(base * 2^(n-1), max)` verified |
| **P0-UNIT-004** | TaskId UUID v4 generation | UNIT | — | Format validation |
| **P0-INT-001** | Task save/find round-trip | INT | — | All fields persisted correctly |
| **P0-INT-002** | Single claim: pending->running | INT | R001 | claimed_by, attempts+1 verified |
| **P0-INT-003** | No pending tasks: claim returns None | INT | R001 | No panic, no block |
| **P0-INT-004** | Concurrent claims: 1 winner (SKIP LOCKED) | INT | R001 | N workers, 1 task, exactly 1 success. Use `multi_thread` |
| **P0-INT-005** | Future scheduled_at: not claimed | INT | R001 | Task invisible until due |
| **P0-INT-006** | Priority ordering: higher first | INT | R001 | Within same queue |
| **P0-INT-007** | Multi-queue isolation | INT | R001 | Queue A workers ignore queue B |
| **P0-INT-008** | Zombie: expired + retries left -> pending | INT | R002 | Sweeper recovery |
| **P0-INT-009** | Zombie: expired + max retries -> failed | INT | R002 | Sweeper permanent failure |
| **P0-INT-010** | Valid lease: sweeper ignores | INT | R002 | No premature recovery |
| **P0-CHAOS-001** | SIGTERM: in-flight complete, zero orphans | CHAOS | R003 | Isolated container |
| **P0-CHAOS-002** | SIGTERM + timeout: leases released | CHAOS | R003 | Isolated container |
| **P0-API-001** | POST /tasks valid -> 201 | API | — | Task in DB as pending |
| **P0-API-002** | POST /tasks invalid -> 422 | API | — | INVALID_PAYLOAD error code |

**Total P0:** 18 tests

---

### P1 (High)

**Criteria:** Core features + Medium risk + Worker pool and error handling

| Test ID | Requirement | Test Level | Risk Link | Notes |
|---------|-------------|------------|-----------|-------|
| **P1-INT-001** | Worker execute -> completed + metric | INT | — | Duration histogram emitted |
| **P1-INT-002** | Handler error -> retry or fail | INT | R002 | last_error recorded |
| **P1-INT-003** | Semaphore: max N concurrent | INT | — | Bounded concurrency |
| **P1-INT-004** | Poll interval respected | INT | — | Configurable cadence |
| **P1-INT-005** | Sweeper recovery counter incremented | INT | — | `zombie_recoveries_total` |
| **P1-INT-006** | Sweeper interval configurable | INT | R002 | Respected in test |
| **P1-UNIT-001** | Domain error type variants | UNIT | — | Display output correct |
| **P1-UNIT-002** | Error From impls preserve context | UNIT | — | Cross-layer conversion |
| **P1-INT-007** | Postgres connection loss -> retry | INT | R008 | No panic on disconnect |
| **P1-API-001** | GET /tasks/{id} -> 200 | API | — | Correct state returned |
| **P1-API-002** | DELETE /tasks/{id} (pending) -> cancelled | API | — | State transition |
| **P1-API-003** | DELETE /tasks/{id} (running) -> 409 | API | — | Conflict |
| **P1-API-004** | GET /tasks with filters | API | — | queue, status, limit, offset |
| **P1-API-005** | POST /tasks body "queue" field | API | — | Task in specified queue |
| **P1-API-006** | GET /health -> 200 | API | — | Health check |
| **P1-API-007** | Body > 1 MiB -> rejected | API | R013 | DefaultBodyLimit enforced |
| **P1-UNIT-003** | TaskRegistry dispatch by kind | UNIT | — | Registered handler found |
| **P1-UNIT-004** | TaskRegistry unregistered -> panic | UNIT | — | Descriptive message |
| **P1-UNIT-005** | Config layering precedence | UNIT | — | defaults->file->.env->env->CLI |
| **P1-UNIT-006** | Missing DATABASE_URL -> error | UNIT | — | Not panic |
| **P1-INT-008** | IronDefer builder -> migrations run | INT | — | Engine ready |
| **P1-INT-009** | engine.enqueue -> task pending | INT | — | DB round-trip |
| **P1-CHAOS-001** | Kill worker -> sweeper recovers all | CHAOS | R001/R002 | 100 tasks, isolated container |
| **P1-CHAOS-002** | Postgres down -> no task loss | CHAOS | R008 | Workers reconnect |
| **P1-CHAOS-003** | Max retries -> failed permanently | CHAOS | R002 | Never re-queued |

**Total P1:** 23 tests (note: 2 extra rows vs matrix due to CHAOS tests)

---

### P2 (Medium)

**Criteria:** Observability, privacy, deployment validation

| Test ID | Requirement | Test Level | Risk Link | Notes |
|---------|-------------|------------|-----------|-------|
| **P2-INT-001** | task_duration_seconds histogram | INT | C3/C5 | OTel harness needed |
| **P2-INT-002** | Pending/running gauges match DB | INT | C5 | Gauge accuracy |
| **P2-INT-003** | Worker pool utilization metric | INT | C5 | active/max ratio |
| **P2-INT-004** | Counter increments correct | INT | C5 | Attempts, failures |
| **P2-UNIT-001** | Default log_payload=false | UNIT | R010 | Config validation |
| **P2-INT-005** | log_payload=false: payload absent | INT | R010 | Tracing output check |
| **P2-INT-006** | log_payload=true: payload present | INT | R010 | Opt-in verified |
| **P2-INT-007** | Migrations create tables+indexes | INT | R014 | Fresh DB |
| **P2-INT-008** | sqlx::migrate! path correct | INT | R014 | Embedded migrations |
| **P2-INT-009** | Pool exhaustion: block, recover | INT | R008 | No panic |
| **P2-INT-010** | Pool metrics emitted | INT | R008 | available, in_use |
| **P2-API-001** | No stack traces in error responses | API | R005 | Security surface |
| **P2-API-002** | No hidden admin endpoints | API | R005 | Security surface |
| **P2-INT-011** | skip_migrations(true) works | INT | — | No migration run |

**Total P2:** 14 tests

---

### P3 (Low)

**Criteria:** Benchmarks, CLI, edge cases

| Test ID | Requirement | Test Level | Notes |
|---------|-------------|------------|-------|
| **P3-BENCH-001** | Throughput >= 10k jobs/sec | BENCH | Criterion, external DB |
| **P3-BENCH-002** | Claim latency P95/P99 | BENCH | High concurrency |
| **P3-INT-001** | CLI: submit task | INT | Task in DB |
| **P3-INT-002** | CLI: inspect queue | INT | Correct output |
| **P3-INT-003** | CLI: invalid config -> error | INT | Clear message |
| **P3-INT-004** | Payload near 1 MiB -> accepted | INT | Boundary test |

**Total P3:** 6 tests

---

## Execution Strategy

**Philosophy:** Run everything in PRs unless expensive or long-running. Rust `cargo test` with testcontainers is fast (~8-12 min for full workspace).

### Every PR: cargo test --workspace (~8-12 min)

**All functional tests** (UNIT + INT + API):
- Domain unit tests: ~1s (pure functions)
- Application unit tests: ~2s (mockall mocks)
- Infrastructure integration tests: ~30-60s (shared testcontainers OnceCell)
- API integration tests: ~60-90s (shared DB, axum test server)
- CI gates: fmt, clippy pedantic, deny, audit, machete, sqlx check, tarpaulin (domain 80%)

### Nightly: Chaos + Full Coverage (~15-25 min)

**Chaos tests** (isolated containers, 3-5 min each):
- P0-CHAOS-001/002 (SIGTERM shutdown)
- P1-CHAOS-001 (worker crash recovery)
- P1-CHAOS-002 (Postgres outage)
- P1-CHAOS-003 (max retries exhaustion)
- Full workspace tarpaulin coverage report

### Weekly/Release: Benchmarks + Deploy Validation (~10-15 min)

- P3-BENCH-001/002: Criterion throughput benchmark (external DATABASE_URL, `release.yml`)
- Docker build validation
- Kubernetes dry-run (`kubectl apply --dry-run=client -k k8s/`)
- CLI integration tests (P3-INT-001/002/003)

---

## QA Effort Estimate

**Test development effort (single developer):**

| Priority | Count | Effort Range | Notes |
|----------|-------|-------------|-------|
| P0 | 18 | ~25-40 hrs | SKIP LOCKED concurrency, chaos shutdown — most complex |
| P1 | 23 | ~20-35 hrs | Worker pool, REST API, config, 3 chaos scenarios |
| P2 | 14 | ~10-20 hrs | OTel metrics (depends on harness), payload privacy |
| P3 | 6 | ~3-8 hrs | Benchmarks (external DB), CLI |
| **Total** | **61** | **~58-103 hrs** | **~3-5 weeks, 1 developer** |

**Assumptions:**
- Includes test design, implementation, debugging, CI integration
- Assumes test infrastructure (OnceCell, chaos helpers) built alongside feature implementation
- P0 tests developed in weeks 1-2; P1 in weeks 2-3; P2/P3 in week 4

---

## Implementation Planning Handoff

| Work Item | Owner | Target | Dependencies |
|-----------|-------|--------|--------------|
| testcontainers OnceCell shared DB helper | Dev | Week 1 | Docker in CI |
| OTel test harness (mock OTLP receiver) | Dev | Week 2 | C3 blocker |
| Sweeper/shutdown test timing configs | Dev | Week 3 | C6, C7 blockers |
| P0 SKIP LOCKED claiming tests | Dev | Week 2 | OnceCell helper ready |
| P0 chaos tests (SIGTERM, shutdown) | Dev | Week 3 | Timing configs ready |
| P1 chaos tests (crash, outage, retries) | Dev | Week 3 | OnceCell + chaos helpers |
| P2 OTel metric tests | Dev | Week 3-4 | OTel harness ready |
| P3 criterion benchmarks | Dev | Week 4 | External DB in CI |

---

## Interworking & Regression

| Component | Impact | Regression Scope | Validation |
|-----------|--------|-----------------|------------|
| **PostgreSQL** | SKIP LOCKED, migrations, queries | sqlx offline cache (`cargo sqlx prepare --check`) | CI gate |
| **Tokio runtime** | Worker pool, shutdown, sweeper | `cargo test --workspace` | PR gate |
| **OTel SDK** | Metrics, logs export | OTel integration tests | Nightly |
| **axum** | REST API, body limit, routing | API integration tests | PR gate |

**Regression strategy:** All existing tests must pass before any PR merges. `cargo test --workspace` is the single regression command.

---

## Appendix A: Code Examples & Tagging

**Rust Test Tagging (module-based):**

```rust
// crates/infrastructure/tests/claiming_test.rs
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_exactly_one_succeeds() {
    let pool = test_pool().await;
    // Insert 1 pending task
    // Spawn N concurrent claim attempts
    // Assert exactly 1 returned Some, rest returned None
}

// crates/api/tests/chaos/worker_crash_test.rs
// Each chaos test uses its OWN isolated Postgres container
#[tokio::test]
async fn worker_killed_mid_execution_sweeper_recovers() {
    let container = Postgres::default().start().await.unwrap();
    // ... isolated test setup
    // Submit 100 tasks, kill worker, verify sweeper recovery
}
```

**Run by crate/module:**

```bash
# All tests
cargo test --workspace

# Domain only (fast, no DB)
cargo test -p iron-defer-domain

# Infrastructure integration (testcontainers)
cargo test -p iron-defer-infrastructure

# Chaos tests only
cargo test -p iron-defer --test 'chaos*'

# Benchmarks (requires DATABASE_URL)
cargo bench -p iron-defer
```

---

## Appendix B: Knowledge Base References

- **Risk Governance**: `risk-governance.md` - Risk scoring methodology (P x I, 1-9 scale)
- **Probability-Impact**: `probability-impact.md` - DOCUMENT/MONITOR/MITIGATE/BLOCK thresholds
- **Test Levels Framework**: `test-levels-framework.md` - UNIT/INT/API/E2E selection rules
- **Test Quality**: `test-quality.md` - Deterministic, isolated, explicit, <300 lines, <1.5 min
- **Test Priorities Matrix**: `test-priorities-matrix.md` - P0-P3 criteria, risk-based adjustments
- **ADR Quality Readiness**: `adr-quality-readiness-checklist.md` - 8-category, 29-criteria NFR framework

---

**Generated by:** BMad TEA Agent
**Workflow:** `bmad-testarch-test-design`
