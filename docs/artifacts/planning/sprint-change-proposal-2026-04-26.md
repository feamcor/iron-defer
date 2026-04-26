# Sprint Change Proposal — Release Cleanup and Readiness Gate

Date: 2026-04-26
Project: iron-defer
Mode: Incremental
Trigger story: 12.2 (Geographic Worker Pinning)
Linked impact stories: 9.1, 10.1, 12.3

## 1) Issue Summary

Growth Phase features are implemented and marked complete, but release readiness is blocked by unresolved review findings and status-tracking drift.

Primary trigger was Story 12.2 where unresolved findings included compilation errors, queue stats API contract ambiguity, and observability risks around region label cardinality. Related unresolved findings in Stories 9.1, 10.1, and 12.3 indicate a systemic closure gap: stories reached `done`/closed tracking states before all critical findings were resolved or formally waived.

Evidence source: `docs/artifacts/implementation/epic-9-10-11-12-retro-2026-04-26.md`.

## 2) Impact Analysis

### Epic Impact

- Epic 12 scope is feature-complete but not release-ready without cleanup closure.
- Epics 9 and 10 are functionally complete but still carry unresolved critical findings (idempotency edge handling and OTLP export verification).
- A new post-growth cleanup epic is required to avoid silently accepting unresolved risk.

### Story Impact

- Story 9.1: unresolved idempotency validation and safety findings.
- Story 10.1: missing OTLP exporter verification path and related trace quality concerns.
- Story 12.2: compile blockers, contract and observability concerns.
- Story 12.3: E2E quality/stability findings (deadlock risk, leak risk, non-deterministic assertions).

### Artifact Conflicts

- PRD conflict: acceptance intent for G4/G8 implies production-grade behavior that unresolved findings currently weaken.
- Architecture conflict: queue stats response-shape compatibility and region-metric label policy are not explicit enough for safe enforcement.
- Epics conflict: no explicit release-readiness gate exists to prevent premature `done` status when critical findings remain open.
- Tracking conflict: sprint/status metadata does not consistently match code-review and story-file states.

### Technical Impact

- Compile and runtime correctness risk (12.2).
- Observability/compliance risk (10.1 OTLP exporter path).
- API compatibility risk (`GET /queues` response semantics in region-aware context).
- Operational risk (unbounded metrics label cardinality).

## 3) Recommended Approach

Selected path: **Option 1 — Direct Adjustment** (recommended).

Why:

- Preserves delivered feature scope and momentum.
- Avoids high-cost rollback while addressing concrete blockers.
- Converts implicit quality expectations into explicit release gates.

Estimate and risk:

- Effort: Medium
- Risk: Medium (reduced to Low after closure + evidence pass)
- Timeline impact: short cleanup sprint / release hardening pass

## 4) Detailed Change Proposals

### A) Epics / Story Plan Update

Artifact: `docs/artifacts/planning/epics.md`

Proposal:

- Add **Epic 13: Release Cleanup & Readiness Gate** immediately after Epic 12.
- Add four stories:
  - **13.1** Close 9.1 + 10.1 critical findings (idempotency safety + OTLP export verification).
  - **13.2** Close 12.2 correctness findings (compile fixes, region validation, queue stats compatibility, metrics label bounds).
  - **13.3** Close 12.3 E2E findings (deadlock/leak/non-determinism fixes, helper completion).
  - **13.4** Tracking and evidence closure (status alignment, release checklist artifacts).

Before → After (summary):

- Before: Growth phase ends at Epic 12 with no explicit release-hardening epic.
- After: Epic 13 introduces an explicit closure gate between feature completion and release readiness.

Rationale:

- Prevents recurring “feature done but risk open” state.
- Makes cleanup work first-class and schedulable.

### B) PRD Clarification Update

Artifact: `docs/artifacts/planning/prd.md`

Proposal:

- Add release-readiness clause under Growth completion quality:
  - A feature is not release-complete while compile-risk, data-loss-risk, security-risk, or contract-breaking review findings remain unresolved or undocumented as accepted waivers.
- Tighten G4 acceptance:
  - OTLP exporter must be configured and verified end-to-end (not only in-memory span assertions).
- Tighten G8 acceptance:
  - Queue stats response contract remains backward-compatible unless a versioned/opt-in mode is introduced.
  - Region metric labels must be bounded/normalized to prevent cardinality explosion.

Before → After (summary):

- Before: quality expectations are implied through ACs.
- After: release gate and compatibility/observability safeguards are explicit and testable.

Rationale:

- Aligns requirements language with observed release blockers.

### C) Architecture Addendum Clarification

Artifact: `docs/artifacts/planning/architecture.md`

Proposal:

- Add explicit OTLP export verification requirement in G4 test strategy.
- Add explicit region label cardinality policy for metrics in G8.
- Add queue stats compatibility contract rule (existing endpoint shape preserved; per-region detail only via versioned or opt-in response mode).
- Add quality-gate note that stories are not marked done while critical findings remain open.

Before → After (summary):

- Before: architecture defines mechanisms but leaves release-safety enforcement partly implicit.
- After: architecture includes enforceable compatibility and readiness constraints.

Rationale:

- Reduces interpretation drift and protects downstream API/ops consumers.

### D) Tracking Hygiene Update

Artifact: `docs/artifacts/implementation/sprint-status.yaml`

Proposal:

- Add Epic 13 and stories 13.1–13.4 with appropriate states (`backlog` or `in-progress` based on approval timing).
- Reconcile story statuses only after closure evidence is attached.

Before → After (summary):

- Before: status metadata can drift from review reality.
- After: status progression follows explicit closure criteria.

Rationale:

- Keeps workflow state trustworthy for handoff and release decisions.

## 5) Implementation Handoff

Scope classification: **Moderate** (backlog reorganization + implementation closure)

Handoff recipients and responsibilities:

- Product Owner + Developer:
  - Add Epic 13 and stories 13.1–13.4.
  - Prioritize Epic 13 immediately after Epic 12.
- Developer:
  - Implement fixes for 9.1, 10.1, 12.2, 12.3 findings.
  - Add/adjust tests for OTLP exporter path and queue stats compatibility.
  - Apply region metrics cardinality controls.
  - Reconcile sprint/story statuses with review evidence.
- Architect:
  - Finalize queue stats API compatibility decision if ambiguity remains.
  - Confirm architecture addendum language for contract stability.
- QA/Test Architect:
  - Validate closure evidence and verify quality gates for release readiness.

Success criteria:

- No unresolved critical findings remain in 9.1, 10.1, 12.2, 12.3.
- OTLP trace export is verified end-to-end.
- Queue stats contract is explicitly stable and tested.
- Region metrics labels are bounded/normalized and validated.
- Tracking artifacts are reconciled and auditable.

## Recommended Routing

Route to: **Product Owner / Developer** for backlog reorganization and execution, with **Architect** consultation for API contract decision.

---

Prepared for approval in Correct Course workflow.
