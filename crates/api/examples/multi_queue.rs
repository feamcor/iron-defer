//! Multi-queue example: two queues with different concurrency settings.
//!
//! Run:
//! ```sh
//! DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer cargo run --example multi_queue
//! ```

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FastTask {
    id: usize,
}

impl Task for FastTask {
    const KIND: &'static str = "fast_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        println!("[fast] processing task {}", self.id);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowTask {
    id: usize,
}

impl Task for SlowTask {
    const KIND: &'static str = "slow_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        println!("[slow] processing task {} (500ms)", self.id);
        tokio::time::sleep(Duration::from_millis(500)).await;
        println!("[slow] done with task {}", self.id);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required.\n\
         Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
         Then: DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer cargo run --example multi_queue",
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    // Build two engine instances sharing the same pool,
    // each configured for a different queue and concurrency.
    // We build them sequentially to ensure migrations run once correctly.
    let fast_engine = Arc::new(
        IronDefer::builder()
            .pool(pool.clone())
            .register::<FastTask>()
            .register::<SlowTask>()
            .queue("fast")
            .worker_config(WorkerConfig {
                concurrency: 4,
                ..WorkerConfig::default()
            })
            .build()
            .await?,
    );

    let slow_engine = Arc::new(
        IronDefer::builder()
            .pool(pool)
            .register::<FastTask>()
            .register::<SlowTask>()
            .queue("slow")
            .worker_config(WorkerConfig {
                concurrency: 1,
                ..WorkerConfig::default()
            })
            .skip_migrations(true)
            .build()
            .await?,
    );

    // Enqueue tasks to both queues (any engine can enqueue to any queue).
    for i in 0..5 {
        fast_engine
            .enqueue("fast", FastTask { id: i })
            .await?;
    }
    for i in 0..3 {
        fast_engine
            .enqueue("slow", SlowTask { id: i })
            .await?;
    }
    println!("Enqueued 5 fast tasks and 3 slow tasks");

    // Start both engine worker pools.
    let token = CancellationToken::new();

    let fast_bg = fast_engine.clone();
    let t1 = token.clone();
    let fast_handle = tokio::spawn(async move { let _ = fast_bg.start(t1).await; });

    let slow_bg = slow_engine.clone();
    let t2 = token.clone();
    let slow_handle = tokio::spawn(async move { let _ = slow_bg.start(t2).await; });

    // Wait for all tasks to complete with timeout.
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(30);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let fast_tasks = fast_engine.list("fast").await?;
        let slow_tasks = fast_engine.list("slow").await?;
        let all_fast_done = !fast_tasks.is_empty() && fast_tasks.iter().all(|t| t.status() == TaskStatus::Completed);
        let all_slow_done = !slow_tasks.is_empty() && slow_tasks.iter().all(|t| t.status() == TaskStatus::Completed);
        if all_fast_done && all_slow_done {
            println!("All tasks completed!");
            break;
        }
        if start.elapsed() > timeout {
            println!("Timed out waiting for tasks to complete");
            break;
        }
    }

    token.cancel();
    // Await background tasks with timeout to avoid deadlock on exit
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        async {
            let _ = tokio::try_join!(fast_handle, slow_handle);
        }
    ).await;
    println!("Done!");
    Ok(())
}
