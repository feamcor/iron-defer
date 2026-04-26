//! Retry and backoff example: a task that fails twice then succeeds on the third attempt.
//!
//! Run:
//! ```sh
//! DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer cargo run --example retry_and_backoff
//! ```

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlakyTask {
    fail_until_attempt: i32,
}

impl Task for FlakyTask {
    const KIND: &'static str = "flaky";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        let current = ctx.attempt().get();
        println!("  attempt {current}: executing FlakyTask (fails until attempt {})", self.fail_until_attempt);
        if current < self.fail_until_attempt {
            return Err(TaskError::ExecutionFailed {
                kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                    source: format!("simulated failure on attempt {current}").into(),
                },
            });
        }
        println!("  attempt {current}: success!");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required.\n\
         Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
         Then: DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer cargo run --example retry_and_backoff",
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let engine = IronDefer::builder()
        .pool(pool)
        .register::<FlakyTask>()
        .queue("default")
        .sweeper_interval(Duration::from_secs(1))
        .build()
        .await?;

    let engine = Arc::new(engine);

    // Submit a task that fails twice, succeeds on attempt 3.
    // Use enqueue_raw to set max_attempts explicitly.
    let record = engine
        .enqueue_raw(
            "default",
            "flaky",
            serde_json::json!({"fail_until_attempt": 3}),
            None,
            None,
            Some(5),
            None,
            None,
        )
        .await?;
    println!("Enqueued task: id={}, max_attempts=5", record.id());

    // Start workers.
    let token = CancellationToken::new();
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_bg.start(token_bg).await;
    });

    // Poll until the task completes or fails permanently.
    let task_id = record.id();
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match engine.find(task_id).await? {
            Some(r) => {
                println!("  status={:?}, attempts={}", r.status(), r.attempts().get());
                match r.status() {
                    TaskStatus::Completed => {
                        println!("Task completed after {} attempts!", r.attempts().get());
                        break;
                    }
                    TaskStatus::Failed => {
                        println!("Task permanently failed after {} attempts.", r.attempts().get());
                        break;
                    }
                    _ => {}
                }
            }
            None => {
                println!("  task {} not found", task_id);
                break;
            }
        }
    }

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_handle).await;
    println!("Done!");
    Ok(())
}
