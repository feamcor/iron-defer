# Documentation Index

Start here for product, operator, and contributor documentation.

See the [Diataxis map](diataxis-map.md) for the full quadrant mapping.

## Quick Start Path

1. Read the [README](../README.md) for project overview and bootstrapping.
2. Run one tutorial from [Tutorials](#tutorials).
3. Use a task-focused guide from [How-to Guides](#how-to-guides).
4. Use [Reference](#reference) for exact command/field/event lookups.

## Tutorials

- [First Task Local](tutorials/first-task-local.md)
- [Embed in Axum](tutorials/embed-in-axum.md)
- [Operate Standalone](tutorials/operate-standalone.md)
- [Retries and Failures](tutorials/retries-and-failures.md)
- [Suspended Workflow](tutorials/suspended-workflow.md)

## How-to Guides

Core:

- [Embedded Library](guides/embedded-library.md)
- [Standalone Binary](guides/standalone-binary.md)
- [REST API](guides/rest-api.md)
- [Configuration](guides/configuration.md)
- [Deployment](guides/deployment.md)
- [Observability](guides/observability.md)
- [Benchmarks](guides/benchmarks.md)

Task-focused:

- [How to Debug Stuck Tasks](guides/how-to-debug-stuck-tasks.md)
- [How to Tune Worker Concurrency](guides/how-to-tune-worker-concurrency.md)
- [How to Enable Audit Trail](guides/how-to-enable-audit-trail.md)
- [How to Run Chaos Tests](guides/how-to-run-chaos-tests.md)
- [How to Rotate Config by Profile](guides/how-to-rotate-config-by-profile.md)
- [How to Harden Production](guides/how-to-harden-production.md)
- [How to Create Idempotent Producers](guides/how-to-create-idempotent-producers.md)
- [How to Use Regions](guides/how-to-use-regions.md)

## Reference

- [Config Reference](reference/config-reference.md)
- [CLI Reference](reference/cli-reference.md)
- [Error Codes](reference/error-codes.md)
- [Metrics Catalog](reference/metrics-catalog.md)
- [Log Events](reference/log-events.md)
- [SQL Queries for Operations](reference/sql-queries-ops.md)

## Explanation

- [Consistency and Delivery Semantics](explanation/consistency-and-delivery-semantics.md)
- [Claiming and Leases](explanation/claiming-and-leases.md)
- [UNLOGGED vs Audit Log](explanation/unlogged-vs-audit-log.md)
- [Embedded vs Standalone](explanation/embedded-vs-standalone.md)
- [Observability Model](explanation/observability-model.md)

## Architecture and Policy Docs

- ADRs: [Architecture Decision Records](adr/)
- Security: [Security Guidelines](guidelines/security.md)
- Logging: [Structured Logging](guidelines/structured-logging.md)
- Quality gates: [Quality Gates](guidelines/quality-gates.md)
- Rust idioms: [Rust Idioms](guidelines/rust-idioms.md)
- Compliance evidence: [Compliance Evidence](guidelines/compliance-evidence.md)
- Postgres reconnection: [Postgres Reconnection](guidelines/postgres-reconnection.md)
