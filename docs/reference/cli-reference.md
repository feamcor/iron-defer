# CLI Reference

## Scope

Command surface for the `iron-defer` binary.

## Canonical Tables/Entries

### Global flags

| Flag | Env | Description |
|---|---|---|
| `--config`, `-c` | `IRON_DEFER_CONFIG` | path to config file |
| `--database-url` | `DATABASE_URL` | Postgres URL |
| `--json` | — | machine-readable output |

`--json` must appear before subcommand.

### `serve`

| Flag | Env | Description |
|---|---|---|
| `--port` | `PORT` | HTTP port |
| `--concurrency` | — | max in-flight tasks |
| `--otlp-endpoint` | — | OTLP endpoint |

### `submit`

| Flag | Required | Description |
|---|---|---|
| `--queue` | yes | queue name |
| `--kind` | yes | task kind |
| `--payload` | yes | JSON payload string |
| `--scheduled-at` | no | RFC3339 time |
| `--priority` | no | i16 priority, default `0` |
| `--max-attempts` | no | i32, minimum `1` |
| `--idempotency-key` | no | queue-scoped idempotency key |

Exit behavior:

- success: `0`
- validation/storage failure: non-zero
- duplicate idempotency key path: dedicated non-zero branch

### `tasks`

| Flag | Description |
|---|---|
| `--queue` | optional queue filter |
| `--status` | optional status filter |
| `--limit` | default `50`, max `100` |
| `--offset` | pagination offset |

Allowed statuses: `pending`, `running`, `completed`, `failed`, `cancelled`, `suspended`.

### `workers`

No extra flags.

### `config validate`

Exit behavior:

- valid config: `0`
- invalid config: non-zero

## Related Docs

- [Configuration Reference](config-reference.md)
- [Standalone Binary Guide](../guides/standalone-binary.md)
