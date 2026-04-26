//! Graceful shutdown integration tests (Story 2.2).
//!
//! Split from `worker_integration_test.rs` for maintainability.
//! Uses `fresh_pool_on_shared_container()` so each test gets its own
//! pool, preventing cross-test pool-state contamination during shutdown
//! (the 60s-sleep tasks in `shutdown_timeout_releases_leases` can leave
//! connections in an unclean state that affects subsequent tests sharing
//! the same pool handle).

mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Task type.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowTask {
    sleep_ms: u64,
}

impl Task for SlowTask {
    const KIND: &'static str = "slow_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// **LOAD-BEARING TEST** — Submit tasks with a short handler (~200ms), start
/// workers, wait for tasks to be claimed, cancel the token. Verify all tasks
/// reach Completed — none orphaned in Running.
#[tokio::test]
async fn shutdown_drains_inflight_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(10),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SlowTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let mut task_ids = Vec::new();
    for _ in 0..3 {
        let saved = engine
            .enqueue(&queue, SlowTask { sleep_ms: 200 })
            .await
            .expect("enqueue");
        task_ids.push(saved.id());
    }

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() <= deadline,
            "timed out waiting for tasks to be claimed"
        );
        let queue_str: String = queue.clone();
        let running: (i64,) =
            sqlx::query_as("SELECT count(*) FROM tasks WHERE queue = $1 AND status = 'running'")
                .bind(&queue_str)
                .fetch_one(pool)
                .await
                .expect("count running");
        if running.0 == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    token.cancel();

    let result = worker_handle.await.expect("join");
    assert!(result.is_ok(), "start() should return Ok after drain");

    let queue_str: String = queue.clone();
    let completed: (i64,) =
        sqlx::query_as("SELECT count(*) FROM tasks WHERE queue = $1 AND status = 'completed'")
            .bind(&queue_str)
            .fetch_one(pool)
            .await
            .expect("count completed");
    assert_eq!(completed.0, 3);

    let running: (i64,) =
        sqlx::query_as("SELECT count(*) FROM tasks WHERE queue = $1 AND status = 'running'")
            .bind(&queue_str)
            .fetch_one(pool)
            .await
            .expect("count running");
    assert_eq!(running.0, 0);
}

/// **LOAD-BEARING TEST** — Submit tasks with a handler that sleeps much longer
/// than the shutdown timeout. After cancellation, the timeout fires and leases
/// are released. Tasks should return to Pending.
#[tokio::test]
async fn shutdown_timeout_releases_leases() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();
    let shutdown_timeout = Duration::from_secs(1);
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        shutdown_timeout,
        concurrency: 2,
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SlowTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let mut task_ids = Vec::new();
    for _ in 0..2 {
        let saved = engine
            .enqueue(&queue, SlowTask { sleep_ms: 60_000 })
            .await
            .expect("enqueue");
        task_ids.push(saved.id());
    }

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() <= deadline,
            "timed out waiting for tasks to be claimed"
        );
        let queue_str: String = queue.clone();
        let running: (i64,) =
            sqlx::query_as("SELECT count(*) FROM tasks WHERE queue = $1 AND status = 'running'")
                .bind(&queue_str)
                .fetch_one(pool)
                .await
                .expect("count running");
        if running.0 == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    let cancel_time = tokio::time::Instant::now();
    token.cancel();

    let result = worker_handle.await.expect("join");
    assert!(result.is_ok(), "start() should return Ok after timeout");

    let elapsed = cancel_time.elapsed();
    // Primary: shutdown should complete within 2× the configured timeout.
    assert!(
        elapsed < shutdown_timeout * 2,
        "shutdown took {elapsed:?}, expected < {:?} (2× shutdown_timeout)",
        shutdown_timeout * 2
    );
    // Safety net: catch catastrophic hangs on slow CI.
    assert!(
        elapsed < Duration::from_secs(30),
        "shutdown should complete within 30s (timeout is {shutdown_timeout:?}), took {elapsed:?}"
    );

    for &id in &task_ids {
        let row: (
            String,
            Option<uuid::Uuid>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) = sqlx::query_as("SELECT status, claimed_by, claimed_until FROM tasks WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_one(pool)
            .await
            .expect("raw SQL");

        assert_eq!(row.0, "pending");
        assert!(row.1.is_none());
        assert!(row.2.is_none());
    }
}
