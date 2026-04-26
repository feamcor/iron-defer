//! Chaos test: SIGTERM graceful shutdown (AC 3, TEA P0-CHAOS-001/002).
//!
//! SIGTERM is simulated via `CancellationToken` cancellation (the
//! architectural shutdown mechanism — Architecture C2, D6.1). In-flight
//! tasks complete or leases are released. Zero orphaned `Running` tasks.
//!
//! Uses its own isolated Postgres container for full independence from
//! the shared `TEST_DB` container used by `shutdown_test.rs`.

mod chaos_common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediumTask {
    sleep_ms: u64,
}

impl Task for MediumTask {
    const KIND: &'static str = "medium_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok(())
    }
}

const TASK_COUNT: usize = 10;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sigterm_no_orphaned_running_tasks() {
    if chaos_common::should_skip() {
        eprintln!("[skip] IRON_DEFER_SKIP_DOCKER_CHAOS set");
        return;
    }

    let (pool, _container, _url, _port) = chaos_common::boot_isolated_chaos_db().await;
    let queue = chaos_common::unique_queue();

    let config = WorkerConfig {
        concurrency: 4,
        poll_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(10),
        lease_duration: Duration::from_secs(30),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<MediumTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build engine");

    for _ in 0..TASK_COUNT {
        engine
            .enqueue(&queue, MediumTask { sleep_ms: 200 })
            .await
            .expect("enqueue");
    }

    let token = CancellationToken::new();
    let engine = Arc::new(engine);
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let engine_handle = tokio::spawn(async move {
        if let Err(e) = engine_bg.start(token_bg).await {
            eprintln!("[engine] exited with error: {e}");
        }
    });

    // Let workers claim some tasks.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Simulate SIGTERM.
    token.cancel();

    tokio::time::timeout(Duration::from_secs(15), engine_handle)
        .await
        .expect("engine should exit within 15s")
        .expect("engine task should not panic");

    // All tasks should be either Completed or Pending (leases released).
    // Zero should be Running with this worker's ID.
    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'running'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count running");
    assert_eq!(
        running, 0,
        "expected zero running tasks after SIGTERM, got {running}"
    );

    let completed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count completed");

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'pending'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count pending");

    assert_eq!(
        completed + pending,
        i64::try_from(TASK_COUNT).expect("fits"),
        "completed ({completed}) + pending ({pending}) should equal {TASK_COUNT}"
    );
}
