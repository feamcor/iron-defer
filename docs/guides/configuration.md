# Configuration Guide

iron-defer composes configuration with figment.

## Precedence

Later layers override earlier layers:

1. Compiled defaults (`AppConfig::default()`)
2. Base config file (`config.toml` or `--config` / `IRON_DEFER_CONFIG`)
3. Optional profile overlay (`config.{IRON_DEFER_PROFILE}.toml`)
4. `.env` file (loaded before env extraction)
5. Environment variables (`IRON_DEFER__` prefix, `__` nesting)
6. CLI flags (highest priority)

## Main sections

### `database`

| Field | Type | Default | Env var |
|---|---|---:|---|
| `database.url` | string | `""` (must be set for runtime DB access) | `DATABASE_URL` or `IRON_DEFER__DATABASE__URL` |
| `database.max_connections` | u32 | `0` (resolved to internal default of 10) | `IRON_DEFER__DATABASE__MAX_CONNECTIONS` |
| `database.test_before_acquire` | bool | `true` | `IRON_DEFER__DATABASE__TEST_BEFORE_ACQUIRE` |
| `database.unlogged_tables` | bool | `false` | `IRON_DEFER__DATABASE__UNLOGGED_TABLES` |
| `database.audit_log` | bool | `false` | `IRON_DEFER__DATABASE__AUDIT_LOG` |

Constraint:

- `unlogged_tables=true` and `audit_log=true` are mutually exclusive.

### `server`

| Field | Type | Default | Env var |
|---|---|---:|---|
| `server.bind_address` | string | `""` (binds as `0.0.0.0`) | `IRON_DEFER__SERVER__BIND_ADDRESS` |
| `server.port` | u16 | `0` | `PORT` or `IRON_DEFER__SERVER__PORT` |
| `server.readiness_timeout_secs` | u64 | `5` | `IRON_DEFER__SERVER__READINESS_TIMEOUT_SECS` |

### `worker`

| Field | Type | Default | Env var |
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

| Field | Type | Default | Env var |
|---|---|---:|---|
| `observability.otlp_endpoint` | string | `""` (disabled) | `IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT` |
| `observability.prometheus_path` | string | `""` | `IRON_DEFER__OBSERVABILITY__PROMETHEUS_PATH` |

### `producer`

| Field | Type | Default | Env var |
|---|---|---:|---|
| `producer.allowed_regions` | string[] | `[]` | `IRON_DEFER__PRODUCER__ALLOWED_REGIONS` |

Behavior:

- Empty list means any valid region string is accepted.
- Non-empty list enforces an allowlist for region-pinned enqueue operations.

## Example `config.toml`

```toml
[database]
url = "postgres://postgres:postgres@localhost:5432/iron_defer"
max_connections = 10

[server]
port = 8080

[worker]
concurrency = 8
poll_interval = "1s"
sweeper_interval = "30s"
shutdown_timeout = "60s"
region = "us-east-1"

[observability]
otlp_endpoint = "http://collector:4317"

[producer]
allowed_regions = ["us-east-1", "eu-west-1"]
```

## Validation

Validate without starting the server:

```sh
iron-defer config validate
```

Use `--json` for machine-readable output.

## UNLOGGED mode

`database.unlogged_tables = true` converts the `tasks` table to PostgreSQL `UNLOGGED` mode.

- Higher write throughput (WAL bypass for that table)
- Data loss risk on crash recovery (table truncation)
- Not replicated to standbys
- Incompatible with `database.audit_log = true`

Use only for workloads where crash durability is explicitly not required.

## Duration format

Duration fields accept human-readable strings such as `500ms`, `5s`, `1m`, `30m`, `1h`.
