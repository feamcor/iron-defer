# Deployment Guide

## Docker Compose

The quickest way to run iron-defer with Postgres:

```sh
docker compose -f docker/docker-compose.yml up
```

This starts Postgres and iron-defer with health-check dependencies. See [`docker/docker-compose.yml`](../../docker/docker-compose.yml).

For development (Postgres only):

```sh
docker compose -f docker/docker-compose.dev.yml up -d
```

## Docker Build

The [`Dockerfile`](../../docker/Dockerfile) uses a multi-stage build:

1. **Builder stage** (rust:1.94-slim) — compiles the release binary with `SQLX_OFFLINE=true`
2. **Runtime stage** (distroless/cc-debian12) — minimal image with just the binary

Build manually:

```sh
docker build -f docker/Dockerfile -t iron-defer .
docker run -e DATABASE_URL=postgres://user:pass@host:5432/db -p 8080:8080 iron-defer
```

## Environment Variables

iron-defer uses the `IRON_DEFER__` prefix with double underscores for nested fields:

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `PORT` | HTTP listen port |
| `IRON_DEFER__DATABASE__URL` | Alternative to DATABASE_URL |
| `IRON_DEFER__DATABASE__MAX_CONNECTIONS` | Connection pool size |
| `IRON_DEFER__SERVER__PORT` | Alternative to PORT |
| `IRON_DEFER__WORKER__CONCURRENCY` | Max in-flight tasks |
| `IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT` | OTLP collector endpoint |

See [Configuration Guide](configuration.md) for the full reference.

## Graceful Shutdown

On SIGTERM/SIGINT, iron-defer:

1. Cancels the `CancellationToken` — stops accepting new work
2. Drains in-flight tasks up to `shutdown_timeout` (default: 30s)
3. If drain times out, the process exits; the sweeper on next startup recovers any orphaned tasks

## Health Probes

| Probe | Endpoint | Behavior |
|-------|----------|----------|
| Liveness | `GET /health` | Always 200 |
| Readiness | `GET /health/ready` | 200 when DB is reachable, 503 otherwise |

### Kubernetes Probe Configuration

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /health/ready
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 5
```

## Deployment Validation

Run the smoke test after deploying:

```sh
./docker/smoke-test.sh http://localhost:8080
```

This validates health probes, task creation, task retrieval, and metrics endpoint. See [`docker/smoke-test.sh`](../../docker/smoke-test.sh).
