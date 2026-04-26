# Metrics Catalog

## Scope

Metric names exposed by OpenTelemetry and Prometheus integration.

## Canonical Tables/Entries

### Execution and reliability

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `iron_defer_task_duration_seconds` | histogram | `queue`, `kind`, `status` | execution duration |
| `iron_defer_task_attempts_total` | counter | `queue`, `kind` | dispatch attempts |
| `iron_defer_task_failures_total` | counter | `queue`, `kind` | terminal failures |
| `iron_defer_zombie_recoveries_total` | counter | `queue` | sweeper recoveries |
| `iron_defer_tasks_suspended_total` | counter | `queue`, `kind` | suspended transitions |
| `iron_defer_suspend_timeout_total` | counter | `queue` | suspend watchdog timeouts |
| `iron_defer_idempotency_keys_cleaned_total` | counter | implementation-defined | cleaned keys |

### Backpressure and pool

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `iron_defer_worker_pool_utilization` | gauge | `queue` | active/concurrency |
| `iron_defer_claim_backoff_total` | counter | `queue`, `saturation` | backoff events |
| `iron_defer_claim_backoff_seconds` | histogram | `queue` | backoff duration |
| `iron_defer_tasks_pending` | observable gauge | `queue` | current pending |
| `iron_defer_tasks_running` | observable gauge | `queue` | current running |
| `iron_defer_pool_connections_total` | observable gauge | none | pool size |
| `iron_defer_pool_connections_idle` | observable gauge | none | idle connections |
| `iron_defer_pool_connections_active` | observable gauge | none | active connections |

### Metrics endpoint

- `GET /metrics`
- content type: `text/plain; version=0.0.4; charset=utf-8`

## Related Docs

- [Observability Guide](../guides/observability.md)
- [Log Events Reference](log-events.md)
