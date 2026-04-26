//! Basic iron-defer example: define a task, enqueue it, and watch it complete.
//!
//! Prerequisites:
//! ```sh
//! docker compose -f docker/docker-compose.dev.yml up -d
//! ```
//!
//! Run:
//! ```sh
//! DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo run --example basic_enqueue
//! ```

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus};
use serde::{Deserialize, Serialize};

/// A simple greeting task that prints a message when executed.
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Connect to Postgres.
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required.\n\
         Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
         Then: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo run --example basic_enqueue",
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    // Step 2: Build the engine, registering our GreetTask handler.
    let engine = IronDefer::builder()
        .pool(pool)
        .register::<GreetTask>()
        .build()
        .await?;

    // Step 3: Enqueue a task.
    let record = engine
        .enqueue(
            "default",
            GreetTask {
                name: "World".into(),
            },
        )
        .await?;
    println!(
        "Enqueued task: id={}, status={:?}",
        record.id(),
        record.status()
    );

    // Step 4: Retrieve the task to see its initial state.
    let found = engine.find(record.id()).await?.expect("task should exist");
    println!("Before processing: status={:?}", found.status());

    // Step 5: Start the worker pool in the background.
    let token = CancellationToken::new();
    let engine = Arc::new(engine);
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let engine_handle = tokio::spawn(async move {
        let _ = engine_bg.start(token_bg).await;
    });

    // Step 6: Wait briefly for the worker to process the task.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Step 7: Retrieve the task again — it should be Completed.
    let found = engine.find(record.id()).await?.expect("task should exist");
    println!("After processing: status={:?}", found.status());
    assert_eq!(found.status(), TaskStatus::Completed);

    // Step 8: Shut down the engine cleanly.
    token.cancel();
    let _ = engine_handle.await;

    println!("Done!");
    Ok(())
}
