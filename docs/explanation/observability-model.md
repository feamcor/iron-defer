# Observability Model

## Question

How are logs, metrics, and traces intended to work together in iron-defer?

## Short Answer

Observability is intentionally layered:

- logs provide per-task lifecycle narratives
- metrics provide aggregate health, saturation, and trend signals
- trace correlation fields connect producer and worker context when available

Operational analysis should correlate by `task_id`, `queue`, `kind`, and time.

## Tradeoffs

- Pros: broad visibility across debugging, alerting, and capacity planning.
- Cons: increased telemetry volume and cardinality management requirements.

## Related Docs

- [Observability Guide](../guides/observability.md)
- [Metrics Catalog](../reference/metrics-catalog.md)
