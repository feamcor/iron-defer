---
title: 'TEA Test Design -> BMAD Handoff Document'
version: '1.0'
workflowType: 'testarch-test-design-handoff'
inputDocuments:
  - 'docs/artifacts/test/test-design-architecture.md'
  - 'docs/artifacts/test/test-design-qa.md'
sourceWorkflow: 'testarch-test-design'
generatedBy: 'TEA Master Test Architect'
generatedAt: '2026-04-04'
projectName: 'iron-defer'
---

# TEA -> BMAD Integration Handoff

## Purpose

This document bridges TEA's test design outputs with BMAD's epic/story decomposition workflow (`create-epics-and-stories`). It provides structured integration guidance so that quality requirements, risk assessments, and test strategies flow into implementation planning.

## TEA Artifacts Inventory

| Artifact | Path | BMAD Integration Point |
|----------|------|------------------------|
| Architecture Test Design | `docs/artifacts/test/test-design-architecture.md` | Epic quality requirements, risk mitigation plans |
| QA Test Design | `docs/artifacts/test/test-design-qa.md` | Story acceptance criteria, test scenarios |
| Risk Assessment | (embedded in both documents) | Epic risk classification, story priority |
| Coverage Strategy | (embedded in QA document) | Story test requirements (61 scenarios) |
| Progress Log | `docs/artifacts/test/test-design-progress.md` | Workflow audit trail |

## Epic-Level Integration Guidance

### Risk References

The following high-priority risks (score >= 6) should appear as **epic-level quality gates**:

| Risk ID | Category | Score | Epic Impact | Required Gate |
|---------|----------|-------|-------------|---------------|
| R005 | SEC | 9 | Deployment/Operations epic | Network isolation documented before GA |
| R001 | TECH/DATA | 6 | Core Engine epic (SKIP LOCKED claiming) | P0-INT-004 concurrent claim test passes |
| R002 | TECH/DATA | 6 | Resilience epic (sweeper/recovery) | All 3 sweeper tests + chaos test pass |
| R003 | TECH/OPS | 6 | Resilience epic (graceful shutdown) | Both SIGTERM chaos tests pass |
| R004 | DATA | 6 | Core Engine epic (lease management) | Idempotency guide reviewed; lease boundary tests pass |

### Quality Gates

| Epic | Gate Criteria | Source |
|------|--------------|--------|
| Core Persistence (Week 1) | P0-INT-001 through P0-INT-007 pass; domain coverage >= 80% | QA doc P0 section |
| Distributed Execution (Week 2) | P0-INT-002-007 concurrent claiming tests pass; P1-INT-001-004 worker pool tests pass | QA doc P0+P1 |
| Resilience & Observability (Week 3) | P0-INT-008-010 sweeper tests pass; P0-CHAOS-001/002 shutdown tests pass; P1-CHAOS-001-003 chaos tests pass | QA doc P0+P1 CHAOS |
| Optimization & Hardening (Week 4) | All P2 tests pass; P3 benchmarks validate >= 10k jobs/sec; R005 documentation complete | QA doc P2+P3 |

## Story-Level Integration Guidance

### P0/P1 Test Scenarios -> Story Acceptance Criteria

The following critical test scenarios MUST be acceptance criteria on their respective stories:

**Task Lifecycle Stories:**
- AC: `TaskStatus` state machine rejects invalid transitions (P0-UNIT-001/002)
- AC: Retry formula produces correct exponential backoff with cap (P0-UNIT-003)

**SKIP LOCKED Claiming Stories:**
- AC: Concurrent N-worker claim on single task: exactly 1 succeeds (P0-INT-004)
- AC: Priority ordering respected within same queue (P0-INT-006)
- AC: Multi-queue isolation: workers never claim from wrong queue (P0-INT-007)

**Sweeper/Recovery Stories:**
- AC: Expired-lease task with retries left is recovered to pending (P0-INT-008)
- AC: Expired-lease task at max attempts transitions to failed (P0-INT-009)
- AC: Active-lease task is NOT recovered by sweeper (P0-INT-010)

**Graceful Shutdown Stories:**
- AC: SIGTERM during execution: zero orphaned Running tasks after exit (P0-CHAOS-001)
- AC: SIGTERM with drain timeout exceeded: leases released before exit (P0-CHAOS-002)

**REST API Stories:**
- AC: POST /tasks with valid payload returns 201 and task in DB (P0-API-001)
- AC: POST /tasks with invalid payload returns 422 with INVALID_PAYLOAD code (P0-API-002)

### Data-TestId Requirements

Not applicable for iron-defer (backend Rust project, no UI). Test identifiers are Rust module paths and function names (e.g., `iron_defer_infrastructure::tests::claiming_test::concurrent_claims_exactly_one_succeeds`).

## Risk-to-Story Mapping

| Risk ID | Category | P x I | Recommended Story/Epic | Test Level |
|---------|----------|-------|----------------------|------------|
| R005 | SEC | 3x3=9 | Deployment/Documentation story | API (P2-API-001/002) |
| R001 | TECH/DATA | 2x3=6 | SKIP LOCKED Claiming Engine story | INT (P0-INT-002-007) |
| R002 | TECH/DATA | 2x3=6 | Sweeper/Reaper Implementation story | INT (P0-INT-008-010) + CHAOS (P1-CHAOS-001) |
| R003 | TECH/OPS | 2x3=6 | Graceful Shutdown story | CHAOS (P0-CHAOS-001/002) |
| R004 | DATA | 3x2=6 | Lease Management / Idempotency story | INT (P0-INT-010) + documentation |
| R006 | PERF | 2x2=4 | Optimization story (Week 4) | BENCH (P3-BENCH-001/002) |
| R008 | OPS | 2x2=4 | Connection Pool Configuration story | INT (P2-INT-009/010) |
| R010 | SEC/DATA | 1x3=3 | Observability/Privacy story | UNIT+INT (P2-UNIT-001, P2-INT-005/006) |

## Recommended BMAD -> TEA Workflow Sequence

1. **TEA Test Design** (`TD`) -> produces this handoff document
2. **BMAD Create Epics & Stories** -> consumes this handoff, embeds quality requirements
3. **TEA ATDD** (`AT`) -> generates acceptance tests per story
4. **BMAD Implementation** -> developers implement with test-first guidance
5. **TEA Automate** (`TA`) -> generates full test suite
6. **TEA Trace** (`TR`) -> validates coverage completeness

## Phase Transition Quality Gates

| From Phase | To Phase | Gate Criteria |
|-----------|----------|---------------|
| Test Design | Epic/Story Creation | All P0 risks have mitigation strategy (5/5 complete) |
| Epic/Story Creation | ATDD | Stories have acceptance criteria from test design |
| ATDD | Implementation | Failing acceptance tests exist for all P0/P1 scenarios |
| Implementation | Test Automation | All acceptance tests pass |
| Test Automation | Release | 61 scenarios pass; domain coverage >= 80%; all 4 chaos tests pass; R005 documented |
