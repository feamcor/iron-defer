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

# Test Design for Architecture: iron-defer

**Purpose:** Architectural concerns, testability gaps, and risk assessment for review by the development team. Serves as a contract on what must be addressed before test development begins.

**Date:** 2026-04-04
**Author:** Fabio (via TEA)
**Status:** Architecture Review Pending
**Project:** iron-defer
**PRD Reference:** `docs/artifacts/planning/prd.md`
**ADR References:** `docs/adr/0001` through `docs/adr/0006`

---

## Executive Summary

**Scope:** System-level test design for iron-defer, a durable background task execution engine for Rust. Covers the complete MVP: task lifecycle, SKIP LOCKED atomic claiming, worker pool, sweeper recovery, graceful shutdown, REST API, CLI, OTel observability, and dual deployment (embedded library + standalone binary).

**Architecture** (from ADRs 0001-0006 + architecture.md):

- 4-crate hexagonal workspace: domain / application / infrastructure / api
- Rust 2024 edition, MSRV 1.94, Tokio runtime, PostgreSQL 14+ via SQLx 0.8
- SKIP LOCKED atomic claiming (River pattern), JoinSet+Semaphore worker pool
- CancellationToken graceful shutdown, OTel OTLP/gRPC metrics+logs
- rustls-only TLS, OpenSSL banned via deny.toml

**Risk Summary:**

- **Total risks**: 15
- **High-priority (score >= 6)**: 5 risks requiring immediate mitigation (1 BLOCK + 4 MITIGATE)
- **Test effort**: ~61 scenarios (~3-5 weeks for 1 developer)

---

## Quick Guide

### BLOCKERS - Team Must Decide

**Pre-implementation items that block test development:**

1. **C3: OTel test harness** - Define a mock OTLP receiver or stdout capture strategy for validating metric/log emission in integration tests. Without this, OTel NFR tests (P2) cannot be written. (recommended owner: Dev)
2. **C6: Sweeper test timing** - Provide a `WorkerConfig` pattern with short sweeper_interval (~1s) and direct lease-expiry DB insertion for testing zombie recovery without waiting 60s. (recommended owner: Dev)
3. **C7: Shutdown test timing** - Provide a `WorkerConfig::with_shutdown_timeout(Duration::from_secs(3))` pattern for chaos/shutdown tests. Default 30s is unusable in test suites. (recommended owner: Dev)

**What we need:** Complete these 3 items during implementation or OTel/chaos test development is blocked.

---

### HIGH PRIORITY - Team Should Validate

1. **R005: Unauthenticated REST API** (score=9) - Document network isolation requirement in deployment guide and README. Verify no hidden admin endpoints. (implementation phase)
2. **R001: SKIP LOCKED correctness** (score=6) - Validate concurrent claiming test uses `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` for deterministic race testing. (implementation phase)
3. **R004: Lease expiry boundary** (score=6) - Document expected at-least-once duplicate behavior in developer guide idempotency section. (implementation phase)

**What we need:** Review and approve these recommendations.

---

### INFO ONLY - Solutions Provided

1. **Test levels**: UNIT (domain) / INT (testcontainers) / API (axum) / CHAOS (isolated containers) / BENCH (criterion)
2. **Test infrastructure**: testcontainers OnceCell shared DB per binary; isolated containers for chaos tests
3. **Execution**: PR ~8-12 min (UNIT+INT+API+CI gates), Nightly ~15-25 min (CHAOS+coverage), Weekly (benchmarks)
4. **Coverage**: 61 scenarios prioritized P0-P3 with risk-based classification
5. **Quality gates**: P0 100%, P1 >= 95%, domain coverage >= 80%, all 4 chaos tests pass

---

## Risk Assessment

**Total risks identified**: 15 (5 high-priority >= 6, 6 medium, 4 low)

### High-Priority Risks (Score >= 6)

| Risk ID | Category | Description | P | I | Score | Mitigation | Owner | Timeline |
|---------|----------|-------------|---|---|-------|------------|-------|----------|
| **R005** | **SEC** | REST API unauthenticated - unauthorized task submission/cancellation | 3 | 3 | **9** | Document network isolation requirement; verify no escalation paths | Dev | Pre-GA |
| **R001** | **TECH/DATA** | SKIP LOCKED claim query bug causes duplicate execution | 2 | 3 | **6** | Concurrent claiming integration tests (P0-INT-004) | Dev | Week 2 |
| **R002** | **TECH/DATA** | Sweeper fails to recover zombie tasks | 2 | 3 | **6** | Zombie recovery tests (P0-INT-008/009/010) + chaos (P1-CHAOS-001) | Dev | Week 3 |
| **R003** | **TECH/OPS** | Graceful shutdown orphans Running tasks | 2 | 3 | **6** | SIGTERM chaos tests (P0-CHAOS-001/002) | Dev | Week 3 |
| **R004** | **DATA** | Parallel execution at lease expiry boundary | 3 | 2 | **6** | Document in idempotency guide; lease expiry boundary tests | Dev | Week 3 |

### Medium-Priority Risks (Score 4-5)

| Risk ID | Category | Description | P | I | Score | Mitigation | Owner |
|---------|----------|-------------|---|---|-------|------------|-------|
| R006 | PERF | SKIP LOCKED throughput degradation under high concurrency | 2 | 2 | 4 | Criterion benchmark (P3-BENCH-001) | Dev |
| R007 | TECH | `.sqlx/` cache stale after query changes | 2 | 2 | 4 | `cargo sqlx prepare --check` in CI | Dev |
| R008 | OPS | Connection pool exhaustion in embedded mode | 2 | 2 | 4 | Pool exhaustion test (P2-INT-009) | Dev |
| R009 | OPS | OTel Collector misconfiguration causes signal loss | 2 | 2 | 4 | OTel integration tests (P2-INT-001-004) | Dev |
| R011 | BUS | Misconfigured max_attempts causes permanent task failure | 2 | 2 | 4 | Max retries chaos test (P1-CHAOS-003) | Dev |
| R015 | TECH/DATA | Sweeper + slow worker both execute same task | 2 | 2 | 4 | Documented as expected at-least-once behavior | Dev |

### Low-Priority Risks (Score 1-3)

| Risk ID | Category | Description | P | I | Score | Action |
|---------|----------|-------------|---|---|-------|--------|
| R010 | SEC/DATA | PII exposure via `log_payload: true` opt-in | 1 | 3 | 3 | Document; test default=false (P2-UNIT-001) |
| R014 | OPS | Migration failure during upgrade | 1 | 3 | 3 | Migration test (P2-INT-007) |
| R012 | SEC | Supply chain CVE in transitive dependency | 1 | 2 | 2 | cargo deny + cargo audit in CI |
| R013 | SEC | Large payload memory pressure | 1 | 2 | 2 | Body limit test (P1-API-007) |

**Risk categories:** TECH (architecture/integration) | SEC (security) | PERF (performance) | DATA (data integrity) | BUS (business logic) | OPS (operations)

---

## Testability Concerns and Architectural Gaps

### ACTIONABLE CONCERNS

#### Blockers to Fast Feedback

| Concern | Impact | What Architecture Must Provide | Owner | Timeline |
|---------|--------|-------------------------------|-------|----------|
| **No OTel test harness** | Cannot validate metric/log emission in tests | Mock OTLP receiver or `opentelemetry-stdout` capture pattern | Dev | Week 2 |
| **Sweeper 60s default** | Zombie recovery tests take minutes | `WorkerConfig` with 1s sweeper_interval + DB lease injection helper | Dev | Week 3 |
| **30s drain timeout** | Shutdown tests take 30s+ each | Configurable short timeout for tests (3s) | Dev | Week 3 |

#### Architectural Improvements Needed

1. **Concurrent claim test pattern**
   - **Current:** No specified multi-thread test runtime
   - **Required:** Use `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` for SKIP LOCKED race tests
   - **Impact if not fixed:** Claiming tests may pass spuriously on single-thread runtime
   - **Owner:** Dev | **Timeline:** Week 2

---

### Testability Assessment Summary

#### What Works Well

- Hexagonal port/adapter design: application layer fully mockable via `mockall` without database
- testcontainers OnceCell shared DB: fast integration test spin-up (~3s), migrations run once
- Chaos test isolation: independent Postgres containers prevent cross-test contamination
- CancellationToken injectable: shutdown testable without OS signals
- All key parameters configurable (concurrency, poll/sweeper intervals, lease duration, shutdown timeout)
- Domain crate: zero external dependencies, fully deterministic parallel tests

#### Accepted Trade-offs (No Action Required)

- **No REST API auth in MVP** - Documented gap; network isolation is the mitigation. Growth phase adds bearer/mTLS/OIDC.
- **W3C trace context deferred** - MVP has per-execution spans only; cross-boundary tracing is Growth phase. Tests scope accordingly.
- **No LISTEN/NOTIFY** - Polling latency (500ms default) is acceptable for MVP. Growth phase optimizes.

---

### Risk Mitigation Plans (Score >= 6)

#### R005: Unauthenticated REST API (Score: 9) - BLOCK

**Mitigation Strategy:**
1. Add explicit "Security: No Authentication in MVP" section to deployment guide
2. Document required network boundary (VPC, service mesh, firewall) for production
3. Test that error responses don't leak internal details (P2-API-001)
4. Verify no hidden admin/debug endpoints (P2-API-002)

**Owner:** Dev | **Timeline:** Pre-GA | **Status:** Planned
**Verification:** Deployment guide review + security surface tests pass

#### R001: SKIP LOCKED Claim Query Correctness (Score: 6) - MITIGATE

**Mitigation Strategy:**
1. Implement P0-INT-004: N concurrent workers claim 1 task, exactly 1 succeeds
2. Test priority ordering (P0-INT-006) and multi-queue isolation (P0-INT-007)
3. Use multi-thread Tokio runtime for deterministic concurrency

**Owner:** Dev | **Timeline:** Week 2 | **Status:** Planned
**Verification:** All P0-INT claiming tests pass under concurrent load

#### R002: Sweeper Zombie Recovery (Score: 6) - MITIGATE

**Mitigation Strategy:**
1. Implement P0-INT-008/009/010: expired lease recovery, max-attempts failure, valid lease ignored
2. Implement P1-CHAOS-001: kill worker mid-execution, verify sweeper recovery
3. Configure short sweeper_interval in tests (C6 blocker)

**Owner:** Dev | **Timeline:** Week 3 | **Status:** Planned
**Verification:** Zombie recovery tests + worker crash chaos test pass

#### R003: Graceful Shutdown (Score: 6) - MITIGATE

**Mitigation Strategy:**
1. Implement P0-CHAOS-001: SIGTERM during execution, zero orphaned Running tasks
2. Implement P0-CHAOS-002: drain timeout exceeded, leases released
3. Configure short shutdown_timeout in tests (C7 blocker)

**Owner:** Dev | **Timeline:** Week 3 | **Status:** Planned
**Verification:** Both SIGTERM chaos scenarios pass

#### R004: Lease Expiry Parallel Execution (Score: 6) - MITIGATE

**Mitigation Strategy:**
1. Document expected at-least-once duplicate behavior in developer guide idempotency section
2. Test that Sweeper only reclaims tasks with expired leases (P0-INT-010)
3. Validate `iron_defer_zombie_recoveries_total` counter increments on recovery (P1-INT-005)

**Owner:** Dev | **Timeline:** Week 3 | **Status:** Planned
**Verification:** Lease boundary tests pass; idempotency guide reviewed

---

### Assumptions and Dependencies

#### Assumptions

1. PostgreSQL 14+ is available in CI via testcontainers (Docker-in-Docker or host Docker)
2. MSRV 1.94 toolchain is available in CI for `cargo check` verification
3. OTel Collector (or mock receiver) will be available for integration tests by Week 2
4. Criterion benchmarks run in a separate CI job with external DATABASE_URL (not in PR pipeline)

#### Dependencies

1. testcontainers Docker access in CI - Required by Week 1
2. OTel test harness decision (C3) - Required by Week 2
3. Sweeper/shutdown test timing patterns (C6, C7) - Required by Week 3

#### Risks to Plan

- **Risk:** Docker-in-Docker not available in CI environment
  - **Impact:** testcontainers-based integration and chaos tests cannot run
  - **Contingency:** Use CI service containers (GitHub Actions `services:`) as alternative

---

**End of Architecture Document**

**Next Steps for Development Team:**
1. Review Quick Guide (BLOCKERS / HIGH PRIORITY / INFO ONLY) and address blockers
2. Assign owners and timelines for high-priority risks (>= 6)
3. Validate assumptions and dependencies
4. Refer to companion QA doc (`test-design-qa.md`) for test scenarios and execution plan
