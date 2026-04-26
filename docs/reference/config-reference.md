# Configuration Reference

## Scope

Canonical configuration keys, defaults, and environment mappings for `iron-defer`.

Resolution order:

1. defaults
2. `config.toml` (or `--config` / `IRON_DEFER_CONFIG`)
3. `config.{IRON_DEFER_PROFILE}.toml`
4. `.env`
5. `IRON_DEFER__...` environment variables
6. CLI flags

## Canonical Tables/Entries

### `database`

| Key | Type | Default | Env |
|---|---|---:|---|
| `database.url` | string | `""` | `DATABASE_URL` or `IRON_DEFER__DATABASE__URL` |
| `database.max_connections` | u32 | `0` (library resolves internal fallback) | `IRON_DEFER__DATABASE__MAX_CONNECTIONS` |
| `database.test_before_acquire` | bool | `true` | `IRON_DEFER__DATABASE__TEST_BEFORE_ACQUIRE` |
| `database.unlogged_tables` | bool | `false` | `IRON_DEFER__DATABASE__UNLOGGED_TABLES` |
| `database.audit_log` | bool | `false` | `IRON_DEFER__DATABASE__AUDIT_LOG` |

Constraint: `database.unlogged_tables` and `database.audit_log` cannot both be true.

### `server`

| Key | Type | Default | Env |
|---|---|---:|---|
| `server.bind_address` | string | `""` | `IRON_DEFER__SERVER__BIND_ADDRESS` |
| `server.port` | u16 | `0` | `PORT` or `IRON_DEFER__SERVER__PORT` |
| `server.readiness_timeout_secs` | u64 | `5` | `IRON_DEFER__SERVER__READINESS_TIMEOUT_SECS` |

### `worker`

| Key | Type | Default | Env |
|---|---|---:|---|
| `worker.concurrency` | u32 | `4` | `IRON_DEFER__WORKER__CONCURRENCY` |
| `worker.log_payload` | bool | `false` | `IRON_DEFER__WORKER__LOG_PAYLOAD` |
| `worker.poll_interval` | duration | `500ms` | `IRON_DEFER__WORKER__POLL_INTERVAL` |
| `worker.sweeper_interval` | duration | `1m` | `IRON_DEFER__WORKER__SWEEPER_INTERVAL` |
| `worker.lease_duration` | duration | `5m` | `IRON_DEFER__WORKER__LEASE_DURATION` |
| `worker.shutdown_timeout` | duration | `30s` | `IRON_DEFER__WORKER__SHUTDOWN_TIMEOUT` |
| `worker.base_delay` | duration | `5s` | `IRON_DEFER__WORKER__BASE_DELAY` |
| `worker.max_delay` | duration | `30m` | `IRON_DEFER__WORKER__MAX_DELAY` |
| `worker.max_claim_backoff` | duration | `30s` | `IRON_DEFER__WORKER__MAX_CLAIM_BACKOFF` |
| `worker.idempotency_key_retention` | duration | `24h` | `IRON_DEFER__WORKER__IDEMPOTENCY_KEY_RETENTION` |
| `worker.suspend_timeout` | duration | `24h` | `IRON_DEFER__WORKER__SUSPEND_TIMEOUT` |
| `worker.region` | string/null | `null` | `IRON_DEFER__WORKER__REGION` |

### `observability`

| Key | Type | Default | Env |
|---|---|---:|---|
| `observability.otlp_endpoint` | string | `""` | `IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT` |
| `observability.prometheus_path` | string | `""` | `IRON_DEFER__OBSERVABILITY__PROMETHEUS_PATH` |

### `producer`

| Key | Type | Default | Env |
|---|---|---:|---|
| `producer.allowed_regions` | string[] | `[]` | `IRON_DEFER__PRODUCER__ALLOWED_REGIONS` |

## Related Docs

- [Configuration Guide](../guides/configuration.md)
- [CLI Reference](cli-reference.md)
