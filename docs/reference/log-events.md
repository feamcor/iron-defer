# Log Events Reference

## Scope

Structured lifecycle events emitted by `iron-defer`.

## Canonical Tables/Entries

### Event names

| Event | Level | Purpose |
|---|---|---|
| `task_enqueued` | info | task accepted and persisted |
| `task_claimed` | info | worker claimed task |
| `task_completed` | info | successful execution |
| `task_failed_retry` | warn | failure with retry scheduled |
| `task_failed_terminal` | error | terminal failure |
| `task_cancelled` | info | pending task cancelled |
| `task_suspended` | info | task suspended for external signal |
| `task_recovered` | info | sweeper recovered zombie task |
| `pool_saturated` | warn | DB pool saturation or connectivity pressure |
| `task_fail_storage_error` | error | infrastructure/storage failure during dispatch |
| `task_fail_panic` | error | handler panic path |
| `task_fail_unexpected_status` | error | unexpected fail-path state |

### Common fields

- `event`
- `task_id`
- `queue`
- `worker_id`
- `kind`
- `attempt`
- `max_attempts`
- `error` (failure events)

Optional fields:

- `payload` (only when `worker.log_payload=true`)
- `scheduled_at`
- `next_scheduled_at`
- `duration_ms`

### Filtering examples

```sh
RUST_LOG=info iron-defer 2>&1 | jq 'select(.event == "task_failed_terminal")'
RUST_LOG=info iron-defer 2>&1 | jq 'select(.task_id == "<uuid>")'
```

## Related Docs

- [Structured Logging](../guidelines/structured-logging.md)
- [Metrics Catalog](metrics-catalog.md)
