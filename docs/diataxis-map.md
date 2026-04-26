# Diataxis Documentation Map

This project documentation follows the Diataxis framework:

- **Tutorials**: learning by doing, step-by-step
- **How-to guides**: solve a specific task in production or development
- **Reference**: factual lookup
- **Explanation**: understand tradeoffs and design

## Tutorials

- `docs/tutorials/first-task-local.md`
- `docs/tutorials/embed-in-axum.md`
- `docs/tutorials/operate-standalone.md`
- `docs/tutorials/retries-and-failures.md`
- `docs/tutorials/suspended-workflow.md`

## How-to Guides

- `docs/guides/how-to-debug-stuck-tasks.md`
- `docs/guides/how-to-tune-worker-concurrency.md`
- `docs/guides/how-to-enable-audit-trail.md`
- `docs/guides/how-to-run-chaos-tests.md`
- `docs/guides/how-to-rotate-config-by-profile.md`
- `docs/guides/how-to-harden-production.md`
- `docs/guides/how-to-create-idempotent-producers.md`
- `docs/guides/how-to-use-regions.md`

## Reference

- `docs/reference/config-reference.md`
- `docs/reference/cli-reference.md`
- `docs/reference/error-codes.md`
- `docs/reference/metrics-catalog.md`
- `docs/reference/log-events.md`
- `docs/reference/sql-queries-ops.md`

## Explanation

- `docs/explanation/consistency-and-delivery-semantics.md`
- `docs/explanation/claiming-and-leases.md`
- `docs/explanation/unlogged-vs-audit-log.md`
- `docs/explanation/embedded-vs-standalone.md`
- `docs/explanation/observability-model.md`

## Existing docs mapped by quadrant

- **How-to**: most files under `docs/guides/`, `docs/guidelines/`
- **Reference**: `docs/guides/rest-api.md`, ADRs in `docs/adr/`
- **Explanation**: ADRs in `docs/adr/`
- **Tutorials**: now added under `docs/tutorials/`

Use `docs/index.md` for the top-level reader flow and quick links.
