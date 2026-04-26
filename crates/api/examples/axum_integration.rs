//! Axum integration example: embed iron-defer inside an existing axum application.
//!
//! Demonstrates the builder pattern with a caller-provided `PgPool` and how to
//! wire a custom axum endpoint that enqueues tasks through the engine.
//!
//! Prerequisites:
//! ```sh
//! docker compose -f docker/docker-compose.dev.yml up -d
//! ```
//!
//! Run:
//! ```sh
//! DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo run --example axum_integration
//! ```
//!
//! Test:
//! ```sh
//! curl -X POST http://localhost:3000/enqueue -H 'Content-Type: application/json' -d '{"name":"World"}'
//! ```

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::post};
use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};

/// A simple greeting task for demonstration purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GreetTask {
    name: String,
}

impl Task for GreetTask {
    const KIND: &'static str = "greet";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        println!("Hello, {}!", self.name);
        Ok(())
    }
}

#[derive(Deserialize)]
struct EnqueueRequest {
    name: String,
}

#[derive(Serialize)]
struct EnqueueResponse {
    task_id: String,
    status: String,
}

/// POST /enqueue — enqueue a `GreetTask` through the shared engine.
async fn enqueue_handler(
    State(engine): State<Arc<IronDefer>>,
    Json(req): Json<EnqueueRequest>,
) -> impl IntoResponse {
    match engine
        .enqueue("default", GreetTask { name: req.name })
        .await
    {
        Ok(record) => (
            StatusCode::CREATED,
            Json(EnqueueResponse {
                task_id: record.id().to_string(),
                status: format!("{:?}", record.status()),
            }),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required.\n\
         Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
         Then: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo run --example axum_integration",
    );

    // Step 1: Create a caller-owned PgPool (the embedded library pattern).
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    // Step 2: Build the iron-defer engine with the caller's pool.
    let engine = Arc::new(
        IronDefer::builder()
            .pool(pool)
            .register::<GreetTask>()
            .build()
            .await?,
    );

    // Step 3: Create a shared CancellationToken for coordinated shutdown.
    let token = CancellationToken::new();

    // Step 4: Spawn the engine's worker pool in the background.
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    tokio::spawn(async move {
        if let Err(e) = engine_bg.start(token_bg).await {
            eprintln!("Engine error: {e}");
        }
    });

    // Step 5: Build the axum router with the engine as shared state.
    let app = Router::new()
        .route("/enqueue", post(enqueue_handler))
        .with_state(engine);

    // Step 6: Start the axum server with graceful shutdown.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("Listening on http://127.0.0.1:3000");
    println!(
        "Try: curl -X POST http://localhost:3000/enqueue -H 'Content-Type: application/json' -d '{{\"name\":\"World\"}}'"
    );

    let token_shutdown = token.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for ctrl-c");
            println!("\nShutting down...");
            token_shutdown.cancel();
        })
        .await?;

    Ok(())
}
