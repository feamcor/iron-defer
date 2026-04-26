# ADR-0004: Async Runtime and Tokio Ecosystem

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

iron-defer is a distributed task execution system. It requires:
- Non-blocking I/O for high-throughput task polling and execution
- HTTP server for the management API
- HTTP client for outbound webhook/callback delivery
- Structured, async-aware logging and tracing
- Developer-friendly async diagnostics during development

The Rust async ecosystem is fragmented — this ADR establishes the canonical stack to avoid mixing incompatible runtimes or duplicating functionality.

## Decision

We standardize on the **Tokio ecosystem** throughout. No mixing of async runtimes (no `async-std`, no `smol`).

## Crate Assignments

| Concern | Crate | Notes |
|---------|-------|-------|
| Async runtime | `tokio` | `full` feature in binaries, explicit features in library crates |
| HTTP server | `axum` | Typed extractors, tower middleware compatibility |
| HTTP client | `reqwest` | Async, TLS, connection pooling |
| Observability | `tracing` + `tracing-subscriber` | Structured logging with span context |
| Dev diagnostics | `tokio-console` | Feature-flagged, never shipped enabled |
| Async traits | `async-trait` | Until stable `async fn in traits` covers all use cases |
| Async utilities | `tokio-util`, `futures` | Stream adapters, `select!`, etc. |

## Tokio

### Feature Selection

```toml
# Binary crates — use full
[dependencies]
tokio = { version = "1", features = ["full"] }

# Library crates — be explicit
[dependencies]
tokio = { version = "1", features = ["sync", "time", "rt"] }
```

Library crates must not pull in the full tokio feature set — this inflates compile times and transitive dependency surfaces for consumers.

### Entry Point

```rust
#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // ...
}
```

For binaries needing fine-grained runtime control:

```rust
fn main() -> color_eyre::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.worker.concurrency)
        .enable_all()
        .build()?
        .block_on(async_main())
}
```

### Task Spawning

```rust
// Prefer structured concurrency — await handles, don't fire-and-forget
let handle = tokio::spawn(async move { work().await });
handle.await??;

// Fire-and-forget only for truly detached background work
// Always name the task for tokio-console visibility
tokio::task::Builder::new()
    .name("task-poller")
    .spawn(poll_loop())?;
```

Never silently drop `JoinHandle` — either `.await` it or explicitly detach with a comment explaining why.

## Axum

### Application State

```rust
// All shared state via Arc — never global statics
#[derive(Clone)]
pub struct AppState {
    pub task_service: Arc<dyn TaskService>,
    pub config: Arc<AppConfig>,
}

let app = Router::new()
    .route("/tasks", post(create_task))
    .route("/tasks/:id", get(get_task))
    .with_state(state);
```

### Handler Convention

```rust
use axum::{extract::{Path, State}, Json, http::StatusCode};

async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>, AppError> {
    let task_id = TaskId::try_from(id.as_str())?;
    let task = state.task_service.find_by_id(&task_id).await?;
    Ok(Json(TaskResponse::from(task)))
}
```

- Use typed extractors — never `Request` unless absolutely necessary
- Return `Result<Json<T>, AppError>` — implement `IntoResponse` for `AppError`
- Keep handlers thin: extract, call service, map response. No business logic in handlers.

### Error Response

```rust
pub struct AppError(pub Box<dyn std::error::Error + Send + Sync>);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = /* map error type to status code */;
        let body = Json(json!({ "error": self.0.to_string() }));
        (status, body).into_response()
    }
}
```

## Reqwest

```rust
// Build client once, reuse everywhere (it's a connection pool)
let client = reqwest::Client::builder()
    .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
    .timeout(Duration::from_secs(30))
    .connect_timeout(Duration::from_secs(10))
    .build()?;

// Store in AppState, clone cheaply (Arc internally)
```

Rules:
- Always set `timeout` and `connect_timeout` — no hanging requests
- Always set `user_agent` — required for responsible API citizens
- Use `reqwest::Error` variants to distinguish network vs. HTTP-level failures
- Never construct a `Client` per-request

## Tracing

### Initialization

```rust
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

fn init_tracing(config: &ObservabilityConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .json();  // structured JSON in production

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}
```

### Instrumentation

```rust
use tracing::{instrument, info, warn, error, Span};

// Instrument all async service and repository methods
#[instrument(skip(self), fields(task_id = %id), err)]
pub async fn execute(&self, id: &TaskId) -> Result<(), TaskError> {
    info!("starting task execution");
    // ...
}
```

Rules:
- `#[instrument]` on every public async method in `application` and `infrastructure` layers
- `skip(self)` always — don't log the whole struct
- Use `fields(...)` to attach key business identifiers to the span
- `err` attribute records the error automatically if the function returns `Err`
- Use `info!` for normal operations, `warn!` for expected degraded states, `error!` for unexpected failures
- Never log secrets: `skip(password)`, `skip(api_key)`, etc.

### Span Propagation (HTTP)

For distributed tracing across services, use `tower-http` with `TraceLayer`:

```rust
use tower_http::trace::TraceLayer;

let app = Router::new()
    // ...
    .layer(TraceLayer::new_for_http());
```

## Tokio Console

Gate behind a feature flag — never compile into production builds:

```toml
# Cargo.toml
[features]
tokio-console = ["dep:console-subscriber"]

[dependencies]
console-subscriber = { version = "0.4", optional = true }
```

```rust
// main.rs
#[cfg(feature = "tokio-console")]
console_subscriber::init();
```

Run locally: `TOKIO_CONSOLE=1 cargo run --features tokio-console`

## Consequences

**Positive:**
- Single async runtime — no compatibility issues between crates
- `axum` + tower middleware gives powerful composability
- Structured tracing integrates with all major log aggregators (Datadog, Loki, etc.)
- `tokio-console` provides real-time task visibility during development

**Negative:**
- Tokio ecosystem lock-in — accepted, as alternatives provide no meaningful advantage for this use case
- `async-trait` macro adds overhead — to be removed when `async fn in traits` stabilizes in stable Rust

## References

- [Tokio documentation](https://tokio.rs)
- [Axum documentation](https://docs.rs/axum)
- [tracing documentation](https://docs.rs/tracing)
- [tokio-console](https://github.com/tokio-rs/console)
- [tower-http](https://docs.rs/tower-http)
