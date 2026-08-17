# ADR-0007: Task Terminology — Zombie Tasks and Claim-Based Attempts

**Status:** Accepted
**Date:** 2026-08-17

---

## Context

While formalizing the ubiquitous language in `CONTEXT.md`, two naming tensions surfaced between prose and code.

First, a task whose lease expired while it was still claimed was described in several places as an "orphaned task" or "stuck task". The code, however, canonizes a different word end to end: `SweeperService` "recovers zombie tasks", the repository port exposes `recover_zombie_tasks()`, the configuration comments define "zombie tasks (running tasks with expired leases)", and the published Prometheus metric is `iron_defer_zombie_recoveries_total`. Worse, "orphaned" is already used in the code for a *different* concept — audit-log rows whose parent task was deleted — so reusing it for tasks would make one word mean two things.

Second, "attempt" had been glossed as a handler invocation. But `AttemptCount` is documented as "number of times a task has been claimed" and is incremented at claim time, not at execution time. A task claimed by a worker that dies before ever invoking the handler still consumes an attempt, and a retry after zombie recovery continues within the same attempt budget.

## Decision

- The canonical term for a task whose lease expired while claimed is **Zombie Task**. Avoid "orphaned task", "stale task", and "stuck task". Recorded in `CONTEXT.md`.
- **Attempt** means one claim of a task by a worker, from the claim to its end — success, failure, cancellation, or lease expiry. **Execute** is the handler invocation within an attempt. Attempts are counted per claim, not per handler invocation.
- **Orphaned** remains reserved for audit-log rows whose parent task no longer exists.

## Considered Options

- **Rename the codebase to "orphaned" for a more formal vocabulary.** Rejected: the metric name `iron_defer_zombie_recoveries_total` is published — renaming breaks dashboards and alerts for no functional gain — and "orphaned" is already overloaded with audit rows.
- **Keep the code's "zombie" but write "orphaned" in docs.** Rejected: drift between the glossary and observability output is worse than a consistent informal term; operators searching logs and metrics for one word must find both everywhere.

## Consequences

- Documentation and glossary vocabulary matches the code and the published metric.
- "Zombie" is informal but now deliberate; it is the only word for lease-expired tasks, and "orphaned" unambiguously means audit rows without a task.
- Anyone reading an attempts counter must understand it counts claims: a task that never executed because its worker died immediately can still show `attempts >= 1`.
- `CONTEXT.md` is the canonical home of this vocabulary; new domain terms are captured there.

## References

- `crates/application/src/services/sweeper.rs` — zombie recovery service
- `crates/application/src/ports/task_repository.rs` — `recover_zombie_tasks`
- `crates/domain/src/model/attempts.rs` — claim-based `AttemptCount`
- `crates/application/src/metrics.rs` — `iron_defer_zombie_recoveries_total`
- `CONTEXT.md` — canonical glossary
