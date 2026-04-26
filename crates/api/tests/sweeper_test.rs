//! Sweeper zombie recovery integration tests (Story 2.1, AC 10).
//!
//! Split from `worker_integration_test.rs` for maintainability.

mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    CancellationToken, IronDefer, QueueName, Task, TaskContext, TaskError, TaskStatus, WorkerConfig,
};
use iron_defer_application::SweeperService;
use iron_defer_domain::WorkerId;
use iron_defer_infrastructure::PostgresTaskRepository;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Task type.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuccessTask {
    n: i32,
}

impl Task for SuccessTask {
    const KIND: &'static str = "success_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// **LOAD-BEARING TEST** (TEA P0-INT-008-010) — Submit a task, claim it with
/// a very short lease (100ms), let the lease expire, run the sweeper, verify
/// the task returns to Pending and is subsequently claimed and completed.
#[tokio::test]
async fn sweeper_recovers_zombie_task() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SuccessTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let saved = engine
        .enqueue(&queue, SuccessTask { n: 99 })
        .await
        .expect("enqueue");
    let task_id = saved.id();

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), false))
        as Arc<dyn iron_defer_application::TaskRepository>;

    let queue_name = QueueName::try_from(queue.as_str()).expect("valid queue");
    let worker_id = WorkerId::new();
    let claimed = repo
        .claim_next(&queue_name, worker_id, Duration::from_millis(100), None)
        .await
        .expect("claim");
    assert!(claimed.is_some(), "should claim the task");
    let claimed_task = claimed.unwrap();
    assert_eq!(claimed_task.id(), task_id);

    tokio::time::sleep(Duration::from_millis(200)).await;

    let sweeper_token = CancellationToken::new();
    let sweeper_token_clone = sweeper_token.clone();
    let sweeper = SweeperService::new(
        repo.clone(),
        Duration::from_millis(50),
        Duration::from_secs(3600),
        sweeper_token.clone(),
    );

    let sweeper_handle = tokio::spawn(async move { sweeper.run().await });

    tokio::time::sleep(Duration::from_millis(150)).await;
    sweeper_token_clone.cancel();
    sweeper_handle.await.expect("join").expect("sweeper ok");

    let row: (
        String,
        Option<uuid::Uuid>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as("SELECT status, claimed_by, claimed_until FROM tasks WHERE id = $1")
        .bind(task_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("raw SQL");

    assert_eq!(row.0, "pending", "task should be reset to pending");
    assert!(row.1.is_none(), "claimed_by should be cleared");
    assert!(row.2.is_none(), "claimed_until should be cleared");

    let worker_token = CancellationToken::new();
    let worker_token_clone = worker_token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(worker_token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() <= deadline,
            "timed out waiting for recovered task to complete"
        );
        let found = engine_ref.find(task_id).await.expect("find").expect("task");
        if found.status() == TaskStatus::Completed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let found = engine_ref.find(task_id).await.expect("find").expect("task");
    assert_eq!(found.status(), TaskStatus::Completed);
    assert_eq!(found.attempts().get(), 2);

    worker_token.cancel();
    worker_handle.await.expect("join").expect("worker ok");
}

/// Sweeper fails exhausted zombie tasks — tasks that have used all retry
/// attempts transition to Failed with the correct `last_error` message.
#[tokio::test]
async fn sweeper_fails_exhausted_zombie() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SuccessTask>()
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let saved = engine
        .enqueue_raw(
            &queue,
            "success_task",
            serde_json::json!({"n": 42}),
            None,
            None,
            Some(1),
            None,
            None,
        )
        .await
        .expect("enqueue_raw");
    let task_id = saved.id();

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), false))
        as Arc<dyn iron_defer_application::TaskRepository>;

    let queue_name = QueueName::try_from(queue.as_str()).expect("valid queue");
    let worker_id = WorkerId::new();
    let claimed = repo
        .claim_next(&queue_name, worker_id, Duration::from_millis(100), None)
        .await
        .expect("claim");
    assert!(claimed.is_some(), "should claim the task");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let sweeper_token = CancellationToken::new();
    let sweeper_token_clone = sweeper_token.clone();
    let sweeper = SweeperService::new(
        repo.clone(),
        Duration::from_millis(50),
        Duration::from_secs(3600),
        sweeper_token.clone(),
    );

    let sweeper_handle = tokio::spawn(async move { sweeper.run().await });

    tokio::time::sleep(Duration::from_millis(150)).await;
    sweeper_token_clone.cancel();
    sweeper_handle.await.expect("join").expect("sweeper ok");

    let row: (String, Option<String>) =
        sqlx::query_as("SELECT status, last_error FROM tasks WHERE id = $1")
            .bind(task_id.as_uuid())
            .fetch_one(pool)
            .await
            .expect("raw SQL");

    assert_eq!(row.0, "failed", "exhausted zombie task should be failed");
    assert_eq!(
        row.1.as_deref(),
        Some("lease expired: max attempts exhausted"),
        "last_error should match architecture spec"
    );
}
