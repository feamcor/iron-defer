# iron-defer

A durable background task queue for Rust, backed by Postgres. iron-defer guarantees at-least-once execution of every submitted task, with automatic retries, jittered backoff, and sweeper-based recovery of orphaned work. It runs as an embedded library inside your application or as a standalone binary with a REST API and CLI.

## Features

- **At-least-once execution** — tasks survive process crashes; the sweeper reclaims orphaned work
- **Postgres-only** — no Redis, no RabbitMQ; one dependency you already run
- **Embedded library + standalone binary** — use `IronDefer::builder()` in your app, or run the `iron-defer` binary with REST API and CLI
- **Typed task handlers** — implement the `Task` trait with `Serialize + Deserialize`; payloads round-trip through JSON
- **Multi-queue support** — named queues with independent concurrency and priority ordering
- **Automatic retries with jittered backoff** — configurable `max_attempts`, exponential delay with randomization
- **OpenTelemetry metrics** — task duration histograms, attempt counters, failure counters, pool utilization gauges
- **Structured JSON logging** — lifecycle events on stdout, payload privacy by default
- **REST API** — `POST /tasks`, `GET /tasks/{id}`, `GET /tasks`, `DELETE /tasks/{id}`, `POST /tasks/{id}/signal`, `GET /tasks/{id}/audit`, `GET /queues`, `GET /metrics`, health probes
- **CLI** — `iron-defer submit`, `iron-defer tasks`, `iron-defer config validate`
- **Graceful shutdown** — in-flight tasks drain before exit; sweeper recovers anything that doesn't finish

## Quick Start

```rust
use iron_defer::{IronDefer, Task, TaskContext, TaskError, CancellationToken};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmailTask { to: String, subject: String }

impl Task for EmailTask {
    const KIND: &'static str = "send_email";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        println!("Sending email to {}: {}", self.to, self.subject);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    let engine = IronDefer::builder()
        .pool(pool)
        .register::<EmailTask>()
        .build()
        .await?;

    engine.enqueue("default", EmailTask {
        to: "user@example.com".into(),
        subject: "Hello from iron-defer".into(),
    }).await?;

    let engine = std::sync::Arc::new(engine);
    let token = CancellationToken::new();
    let bg = engine.clone();
    let t = token.clone();
    tokio::spawn(async move { bg.start(t).await });

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    token.cancel();
    Ok(())
}
```

## Getting Started

iron-defer is not published to crates.io. Add it as a path or git dependency:

```toml
[dependencies]
iron-defer = { git = "https://github.com/feamcor/iron-defer" }
```

Start Postgres:

```sh
docker compose -f docker/docker-compose.dev.yml up -d
```

Run the basic example:

```sh
DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
  cargo run --example basic_enqueue
```

## Examples

| Example | Description |
|---------|-------------|
| [`basic_enqueue`](crates/api/examples/basic_enqueue.rs) | Define a task, enqueue, verify completion |
| [`axum_integration`](crates/api/examples/axum_integration.rs) | Embed iron-defer in an axum web server |
| [`retry_and_backoff`](crates/api/examples/retry_and_backoff.rs) | Configure retries, observe failure and recovery |
| [`multi_queue`](crates/api/examples/multi_queue.rs) | Multiple queues with different concurrency |

Run any example with:

```sh
DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
  cargo run --example <name>
```

## Deployment

**Embedded mode:** Add `iron-defer` as a library dependency, build the engine with `IronDefer::builder()`, and call `engine.start(token)` to spawn workers inside your process.

**Standalone mode:** Run the `iron-defer` binary. It starts an HTTP server (REST API + health probes + metrics), a worker pool, and a sweeper. Configure via `config.toml`, environment variables, or CLI flags.

```sh
DATABASE_URL=postgres://... iron-defer serve --port 8080
```

## Configuration

iron-defer uses a layered configuration chain: defaults < `config.toml` < `config.{profile}.toml` < environment variables < CLI flags.

Key settings: `DATABASE_URL`, `PORT`, worker `concurrency`, `poll_interval`, `sweeper_interval`, `shutdown_timeout`.

**UNLOGGED table mode:** Set `database.unlogged_tables = true` (or `IRON_DEFER__DATABASE__UNLOGGED_TABLES=true`) for high-throughput non-durable workloads. **Warning:** data in the tasks table will be lost on PostgreSQL crash recovery. Mutually exclusive with `database.audit_log`. See [Configuration Guide](docs/guides/configuration.md#unlogged-mode).

Validate configuration:

```sh
iron-defer config validate
```

## API Reference

The REST API serves an OpenAPI 3.1 spec at `GET /openapi.json`. Key endpoints:

- `POST /tasks` — create a task
- `GET /tasks` — list tasks (requires `queue` or `status`; paginated)
- `GET /tasks/{id}` — get a single task
- `DELETE /tasks/{id}` — cancel a task
- `POST /tasks/{id}/signal` — resume suspended task with optional signal payload
- `GET /tasks/{id}/audit` — fetch task audit log entries
- `GET /queues` — queue statistics
- `GET /health` / `GET /health/ready` — liveness and readiness probes
- `GET /metrics` — Prometheus metrics

## Documentation

See the [Documentation Map](docs/index.md) for the full guide → example → test chain.

**Guides:**
- [Embedded Library](docs/guides/embedded-library.md) — builder API, caller-provided pool, migration opt-out
- [Standalone Binary](docs/guides/standalone-binary.md) — Docker, CLI subcommands, global flags
- [REST API Reference](docs/guides/rest-api.md) — all endpoints, request/response examples, error codes
- [Deployment](docs/guides/deployment.md) — Docker Compose, Kubernetes probes, graceful shutdown
- [Observability](docs/guides/observability.md) — OTel metrics, Prometheus, structured logging
- [Configuration](docs/guides/configuration.md) — all config fields, figment chain, env vars

**Reference:**
- [Structured Logging](docs/guidelines/structured-logging.md)
- [Compliance Evidence](docs/guidelines/compliance-evidence.md)
- [Security](docs/guidelines/security.md)

## License

Licensed under MIT OR Apache-2.0.
