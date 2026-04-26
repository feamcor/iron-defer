# Embedded Library Guide

Use iron-defer as a library inside your existing Tokio application. The engine runs in-process — no separate binary or sidecar needed.

## Prerequisites

- Rust 1.94+
- A running PostgreSQL instance
- `iron-defer` added as a dependency (path or git)

## Quick Start

```rust
use iron_defer::{IronDefer, Task, TaskContext, TaskError, CancellationToken};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MyTask { data: String }

impl Task for MyTask {
    const KIND: &'static str = "my_task";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        println!("Processing: {}", self.data);
        Ok(())
    }
}
```

## Builder API

Build the engine with `IronDefer::builder()`:

```rust
let pool = sqlx::PgPool::connect(&database_url).await?;
let engine = IronDefer::builder()
    .pool(pool)                          // Required: caller-provided PgPool
    .register::<MyTask>()                // Register task handler(s)
    .queue("default")                    // Queue for workers to poll
    .worker_config(config)               // Optional: concurrency, timeouts
    .sweeper_interval(Duration::from_secs(60)) // Optional: zombie recovery interval
    .shutdown_timeout(Duration::from_secs(30)) // Optional: drain timeout
    .skip_migrations(true)               // Optional: if you manage migrations separately
    .build()
    .await?;
```

### Builder Methods

| Method | Description |
|--------|-------------|
| `.pool(pool)` | **Required.** Caller-provided `sqlx::PgPool` |
| `.register::<T>()` | Register a `Task` implementor. Call once per task type |
| `.queue(name)` | Queue name for workers to poll (default: none — workers won't start without this) |
| `.worker_config(config)` | `WorkerConfig` with concurrency, poll interval, timeouts |
| `.database_config(config)` | `DatabaseConfig` for connection tuning |
| `.sweeper_interval(dur)` | How often the sweeper checks for zombie tasks |
| `.shutdown_timeout(dur)` | Maximum time to drain in-flight tasks on shutdown |
| `.readiness_timeout(dur)` | Timeout for `SELECT 1` readiness check |
| `.metrics(metrics)` | OTel metrics handles (see [Observability Guide](observability.md)) |
| `.prometheus_registry(reg)` | Prometheus registry for `/metrics` endpoint |
| `.skip_migrations(bool)` | Skip automatic migration on build (default: false) |
| `.build()` | Build the engine (runs migrations unless skipped) |

## Caller-Provided Pool

iron-defer does not create its own pool. You pass a `sqlx::PgPool` to the builder, giving you full control over connection limits, timeouts, and TLS configuration:

```rust
let pool = sqlx::postgres::PgPoolOptions::new()
    .max_connections(10)
    .connect(&database_url)
    .await?;
```

## Starting Workers

Call `engine.start(token)` to spawn the worker pool and sweeper:

```rust
let engine = std::sync::Arc::new(engine);
let token = CancellationToken::new();

let bg = engine.clone();
let t = token.clone();
tokio::spawn(async move { bg.start(t).await });
```

Cancel the token to trigger graceful shutdown. In-flight tasks drain up to `shutdown_timeout`.

## Migration Opt-Out

By default, `build()` runs SQL migrations. If your application manages migrations separately (e.g., via a CI pipeline), use `.skip_migrations(true)`.

## Examples

- [`basic_enqueue.rs`](../../crates/api/examples/basic_enqueue.rs) — minimal lifecycle
- [`axum_integration.rs`](../../crates/api/examples/axum_integration.rs) — embedding in an axum server
- [`multi_queue.rs`](../../crates/api/examples/multi_queue.rs) — multiple queues with different concurrency
