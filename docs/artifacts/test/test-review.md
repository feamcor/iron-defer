---
stepsCompleted: ['step-01-load-context', 'step-02-discover-tests', 'step-03f-aggregate-scores', 'step-04-generate-report']
lastStep: 'step-04-generate-report'
lastSaved: '2026-04-21'
workflowType: 'testarch-test-review'
inputDocuments:
  - 'docs/artifacts/test/test-design-architecture.md'
  - 'docs/artifacts/implementation/sprint-status.yaml'
  - 'knowledge/test-quality.md'
  - 'knowledge/data-factories.md'
  - 'knowledge/test-levels-framework.md'
  - 'knowledge/test-healing-patterns.md'
  - 'knowledge/timing-debugging.md'
  - 'knowledge/selective-testing.md'
---

# Test Quality Review: iron-defer

**Date:** 2026-04-21
**Reviewer:** TEA (Master Test Architect)
**Project:** iron-defer
**Scope:** Suite (all tests)

---

## Step 1: Context Summary

### Project Overview

- **Type:** Durable background task execution engine for Rust
- **Architecture:** 4-crate hexagonal workspace (domain / application / infrastructure / api)
- **Stack:** Rust 2024, MSRV 1.94, Tokio, PostgreSQL 14+ via SQLx 0.8, OTel observability
- **Status:** All epics complete (1A-5: Task Persistence, Claiming & Execution, Resilience & Recovery, Observability & Compliance, Operator Interface, Production Readiness).

### Test Infrastructure

- **Framework:** Rust's built-in `#[test]` / `#[tokio::test]`
- **Integration:** `testcontainers` + `testcontainers-modules` (Postgres)
- **Mocking:** `mockall`
- **Tracing:** `tracing-test`
- **Timing:** `tokio::time::pause()` via `test-util` feature
- **Benchmarks:** `criterion`
- **Test design doc:** Available (`test-design-architecture.md`) — defines 61 scenarios, P0-P3 priority, risk-based classification

### Changes Since Prior Review (2026-04-21 earlier run)

- **File splits completed:** `otel_compliance_test.rs` → 3 files, `worker_integration_test.rs` → 3 files (F1/F2 resolved)
- **New test files:** `cli_test.rs`, `config_validation_test.rs`, `chaos_*.rs` (4 files), `pool_exhaustion_test.rs`, `sweeper_counter_test.rs`
- **New unit tests:** `poll_interval_respected`, `sweeper_interval_configurable_and_respected` (deterministic timing via `start_paused`)
- **Test count:** 249 pass / 0 fail / 3 ignored (up from 184 at first review)

### Knowledge Fragments Loaded (Core)

- test-quality.md (Definition of Done)
- data-factories.md (Factory patterns)
- test-levels-framework.md (Unit/Integration/E2E selection)
- test-healing-patterns.md (Failure pattern catalog)
- timing-debugging.md (Race condition fixes)
- selective-testing.md (Tag/filter strategies)

---

## Step 2: Test Discovery & Parse Results

### Summary

| File | Lines | Framework | Test Count | Level |
|------|-------|-----------|------------|-------|
| `api/tests/rest_api_test.rs` | 1127 | `#[tokio::test]` | 28 | API/Integration |
| `infra/tests/task_repository_test.rs` | 694 | `#[tokio::test]` | 17 | Integration |
| `api/tests/otel_metrics_test.rs` | 424 | `#[tokio::test]` | 4 | Integration |
| `api/tests/worker_pool_test.rs` | 386 | `#[tokio::test]` | 4 | Integration |
| `api/tests/integration_test.rs` | 353 | `#[tokio::test]` | 9 | Integration |
| `api/tests/audit_trail_test.rs` | 319 | `#[tokio::test]` | 1 | Integration |
| `api/tests/otel_lifecycle_test.rs` | 250 | `#[tokio::test]` + `#[traced_test]` | 2 | Integration |
| `api/tests/shutdown_test.rs` | 213 | `#[tokio::test]` | 2 | Integration |
| `api/tests/sweeper_test.rs` | 214 | `#[tokio::test]` | 2 | Integration |
| `api/tests/cli_test.rs` | 175 | `#[tokio::test]` | 6 | Integration |
| `api/tests/metrics_test.rs` | 179 | `#[tokio::test]` | 1 | Integration |
| `api/tests/chaos_worker_crash_test.rs` | 169 | `#[tokio::test]` | 1 | Chaos |
| `api/tests/chaos_db_outage_test.rs` | 162 | `#[tokio::test]` + `#[traced_test]` | 1 | Chaos |
| `api/tests/otel_counters_test.rs` | 141 | `#[tokio::test]` + `#[traced_test]` | 1 | Integration |
| `api/tests/chaos_max_retries_test.rs` | 136 | `#[tokio::test]` | 1 | Chaos |
| `api/tests/sweeper_counter_test.rs` | 122 | `#[tokio::test]` | 1 | Integration |
| `api/tests/chaos_sigterm_test.rs` | 122 | `#[tokio::test]` | 1 | Chaos |
| `api/tests/observability_test.rs` | 100 | `#[tokio::test]` + `#[traced_test]` | 1 | Integration |
| `api/tests/pool_exhaustion_test.rs` | 79 | `#[tokio::test]` | 1 | Integration |
| `api/tests/config_validation_test.rs` | 40 | `#[tokio::test]` | 2 | Integration |
| `infra/tests/tracing_privacy_test.rs` | 83 | `#[tokio::test]` + `#[traced_test]` | 1 | Integration |
| `infra/tests/init_tracing_test.rs` | 35 | `#[test]` (sync) | 1 | Unit/Integration |
| `api/tests/common/mod.rs` | ~185 | N/A (helper) | 0 | Shared fixture |
| `api/tests/common/otel.rs` | ~249 | N/A (helper) | 0 | Shared fixture |
| `infra/tests/common/mod.rs` | ~70 | N/A (helper) | 0 | Shared fixture |

**Totals:** 22 test files, 88 integration test functions, ~5,600 lines of integration test code

**Inline Unit Tests:** 14 modules with `#[cfg(test)]`, ~182 unit test functions

---

## Step 3: Quality Evaluation Results

### Overall Score: 92/100 (Grade: A-)

| Dimension | Score | Grade | Weight | Contribution |
|-----------|-------|-------|--------|-------------|
| Determinism | 95/100 | A | 30% | 28.5 |
| Isolation | 95/100 | A | 30% | 28.5 |
| Maintainability | 84/100 | B | 25% | 21.0 |
| Performance | 90/100 | A- | 15% | 13.5 |

> Coverage is excluded from `test-review` scoring. Use `trace` for coverage analysis and gates.

### Improvement from Prior Review

| Dimension | Prior Score | Current Score | Delta | Cause |
|-----------|-----------|---------------|-------|-------|
| Determinism | 92 | 95 | +3 | `start_paused` timing tests eliminate sleep-based nondeterminism |
| Isolation | 93 | 95 | +2 | Pool exhaustion test uses dedicated tiny pool; sweeper counter isolated binary |
| Maintainability | 78 | 84 | +6 | F1/F2 file splits completed; no files over guideline except rest_api_test |
| Performance | 88 | 90 | +2 | File splits removed Mutex serializer; isolated binaries enable parallelism |
| **Overall** | **88** | **92** | **+4** | |

### Violation Summary

| Severity | Count | Impact |
|----------|-------|--------|
| HIGH | 1 | File length (rest_api_test.rs 1127 lines) |
| MEDIUM | 4 | Static atomics, unsafe env var, task_repository_test growth |
| LOW | 3 | Minor: boilerplate, inherent slow chaos tests |
| **TOTAL** | **8** | (down from 13 in prior review) |

---

## Step 4: Detailed Findings & Report

### Exemplary Practices (Maintained + New)

**Maintained from prior review:**

1. **`unique_queue()` isolation pattern** — UUID-scoped queue names prevent cross-test pollution.
2. **`OnceCell<TestDb>` shared container** — One Postgres per integration test binary.
3. **LOAD-BEARING TEST markers** — Critical tests annotated with contracts.
4. **Raw SQL verification** — Direct DB assertions, not just API-level.
5. **Positive + negative control** — Observability tests use dual assertions.
6. **`TestServer` with `Drop` cleanup** — Graceful shutdown on test panics.
7. **Per-test Prometheus registry** — No sample leakage between OTel tests.
8. **Story/AC reference comments** — Traceability chain from test to requirement.

**New exemplary practices:**

9. **`start_paused` deterministic timing** — `poll_interval_respected` and `sweeper_interval_configurable_and_respected` use `tokio::time::pause()` + `advance()` to test interval behavior without any sleeps. This is the gold standard for testing time-dependent behavior in Tokio.

10. **Pool exhaustion cycle test** — `pool_exhaustion_blocks_then_recovers` creates a deliberately tiny pool (2 connections), exhausts it, proves the engine blocks without panicking, then recovers. Clean test of a hard-to-reproduce production scenario.

11. **Chaos test isolation via `chaos_common.rs`** — Shared boot helper for chaos tests that creates dedicated containers, preventing any cross-test state leakage.

---

### Critical Findings (HIGH Severity)

#### F1: `rest_api_test.rs` — 1127 lines

**Category:** Maintainability / File Too Long
**Impact:** Exceeds the 300-line guideline by 3.7x. Contains 28 test functions spanning Stories 1B.3, 4.1, 4.2, and security surface tests. Cross-story navigation requires extensive scrolling.

**Recommendation:** Split by story/feature area:
- `rest_api_crud_test.rs` — Story 1B.3 (POST/GET basic CRUD, ~300 lines)
- `rest_api_cancel_test.rs` — Story 4.1 (DELETE cancel tests, ~250 lines)
- `rest_api_list_test.rs` — Story 4.2 (GET /tasks list + filters, ~250 lines)
- `rest_api_security_test.rs` — Security surface (body limit, error leak, hidden endpoints, ~150 lines)
- `rest_api_extras_test.rs` — Health, queue stats, OpenAPI spec (~200 lines)

Move `TestServer` and `create_tasks` helper into a shared module.

---

### Warnings (MEDIUM Severity)

#### W1: Static Atomic Counters — Shared Mutable State

**Files:** `worker_pool_test.rs:42-64`, `otel_lifecycle_test.rs:58`, `chaos_db_outage_test.rs:20`
**Impact:** `CONCURRENCY_ACTIVE`, `CONCURRENCY_PEAK`, `RETRY_ATTEMPT`, `RETRY_ONCE_EXECUTE_CALLS`, `COUNTER` require manual reset. Safe because each file is a separate binary, but a maintenance trap if tests are ever merged.
**Recommendation:** Document the reset contract. For new tests, prefer `Arc<AtomicU32>` per-test state (as demonstrated by the unit tests in `worker.rs`).

#### W2: `task_repository_test.rs` — 694 lines

**File:** `crates/infrastructure/tests/task_repository_test.rs`
**Impact:** 17 tests in one file. Approaching the guideline threshold. Clean and well-organized currently, but growth from new repository methods would push it past comfort.
**Recommendation:** Split into `task_crud_test.rs` and `task_claiming_test.rs` when the next batch of repository methods is added.

#### W3: Unsafe `std::env::set_var`

**File:** `otel_metrics_test.rs:147`
**Impact:** `unsafe { std::env::set_var("IRON_DEFER_TASK_COUNT_REFRESH_MS", "200") }` mutates the process environment. Guarded by `OnceLock` but technically unsafe in Rust 2024.
**Recommendation:** Add a `task_count_refresh_interval` field to `WorkerConfig` to make this configurable without env var mutation.

#### W4: `rest_api_test.rs` Helper Functions

**File:** `rest_api_test.rs`
**Impact:** `TestServer` struct and `create_tasks` helper are embedded in the test file. When the file is split (F1), these would need to be duplicated or extracted.
**Recommendation:** Extract `TestServer` to `common/rest.rs` and `create_tasks` to a shared helper, ahead of the file split.

---

### Low Severity Notes

- **L1:** `chaos_db_outage_test.rs` has inherent ~3s runtime (pool exhaustion timeout). Acceptable for chaos category. Has timeout guards.
- **L2:** Some task type definitions (`RestTestTask`, `SlowTask`, `OtelSleepTask`) are duplicated across test files. Acceptable — each file is a separate binary.
- **L3:** Repetitive builder boilerplate (~8 lines per test for Docker-skip guard + engine builder). Readable and explicit; extraction is optional.

---

## Quality Assessment Summary

**Overall: 92/100 (A-) — Strong test suite with excellent fundamentals.**

The iron-defer test suite demonstrates mature testing practices for a Rust backend project. The +4 point improvement from the prior review (88 → 92) reflects:

- **F1/F2 resolution:** Splitting `otel_compliance_test.rs` and `worker_integration_test.rs` removed the two prior HIGH findings and naturally eliminated the Mutex serializer, improving both maintainability and performance.

- **Deterministic timing:** The new `start_paused` tests (`poll_interval_respected`, `sweeper_interval_configurable_and_respected`) are the cleanest possible approach to testing interval-based behavior — zero sleeps, zero flakiness, instant execution.

- **Complete TEA coverage:** All 61 designed scenarios now have FULL test coverage (100% across P0-P3). The trace workflow confirmed PASS with no gaps.

**Remaining action item:** Split `rest_api_test.rs` (1127 lines, 28 tests) to bring the suite to zero HIGH findings.

### Next Recommended Actions

1. **Split `rest_api_test.rs`** (HIGH — reduces file size by ~75%, last remaining HIGH finding)
2. **Extract `TestServer` to `common/rest.rs`** (MEDIUM — enables the split and removes future duplication)
3. **Add `task_count_refresh_interval` to `WorkerConfig`** (MEDIUM — removes `unsafe` env var mutation)
4. **Document static atomic reset contracts** (MEDIUM — prevents future maintenance trap)
