# Standalone Binary Guide

Run iron-defer as a standalone binary with HTTP server, worker pool, and CLI.

## Running the Server

```sh
DATABASE_URL=postgres://user:pass@localhost:5432/mydb iron-defer serve --port 8080
```

If no subcommand is given, `serve` is the default. The server starts:
- HTTP API (REST endpoints, health probes, metrics)
- Worker pool (polls configured queue)
- Sweeper (recovers zombie tasks)

### Serve Flags

| Flag | Env Var | Description |
|------|---------|-------------|
| `--port` | `PORT` | HTTP listen port |
| `--concurrency` | | Max simultaneous in-flight tasks |
| `--otlp-endpoint` | | OTLP collector endpoint (empty = disabled) |

## CLI Subcommands

### `iron-defer submit`

Submit a task to a queue:

```sh
iron-defer submit --queue payments --kind webhook --payload '{"url":"https://example.com"}'
```

| Flag | Description |
|------|-------------|
| `--queue` | Target queue name |
| `--kind` | Task type discriminator |
| `--payload` | JSON payload string |
| `--scheduled-at` | Future execution time (ISO 8601) |
| `--priority` | Priority (higher = picked sooner, default: 0) |
| `--max-attempts` | Maximum retry attempts (default: server default of 3) |
| `--idempotency-key` | Reuse-safe submission key scoped to queue |

### `iron-defer tasks`

List and filter tasks:

```sh
iron-defer tasks --queue payments --status pending
```

| Flag | Description |
|------|-------------|
| `--queue` | Filter by queue name |
| `--status` | Filter by status (pending, running, completed, failed, cancelled, suspended) |
| `--limit` | Max results (default: 50, max: 100) |
| `--offset` | Pagination offset |

Supported status values: `pending`, `running`, `completed`, `failed`, `cancelled`, `suspended`.

### `iron-defer workers`

Show active worker status:

```sh
iron-defer workers
```

### `iron-defer config validate`

Validate configuration without starting:

```sh
iron-defer config validate
```

## Global Flags

| Flag | Env Var | Description |
|------|---------|-------------|
| `--config` / `-c` | `IRON_DEFER_CONFIG` | Path to TOML config file |
| `--database-url` | `DATABASE_URL` | PostgreSQL connection string |
| `--json` | | Emit output as JSON instead of tables |

For command-specific behavior and exit codes, run `iron-defer <subcommand> --help`.

The `--json` flag is global and must appear **before** the subcommand:

```sh
iron-defer --json submit --queue test --kind ping --payload '{}'
```

## Docker

Build and run with Docker:

```sh
docker compose -f docker/docker-compose.yml up
```

Or run the image directly:

```sh
docker run -e DATABASE_URL=postgres://... -p 8080:8080 iron-defer
```

See [`docker/Dockerfile`](../../docker/Dockerfile) for the multi-stage build and [`docker/docker-compose.yml`](../../docker/docker-compose.yml) for the full setup.

For development, use [`docker/docker-compose.dev.yml`](../../docker/docker-compose.dev.yml) which starts only Postgres.
