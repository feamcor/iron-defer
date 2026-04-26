//! Chaos test: Max retries exhausted (AC 4, TEA P1-CHAOS-003).
//!
//! A task that always fails exhausts all retry attempts. The task
//! transitions to `Failed` and is never re-queued as `Pending`.

mod chaos_common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FailingTask {
    message: String,
}

impl Task for FailingTask {
    const KIND: &'static str = "failing_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Err(TaskError::ExecutionFailed {
            kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                source: self.message.clone().into(),
            },
        })
    }
}

const MAX_ATTEMPTS: i32 = 3;

#[tokio::test]
async fn max_retries_exhausted_transitions_to_failed() {
    if chaos_common::should_skip() {
        eprintln!("[skip] IRON_DEFER_SKIP_DOCKER_CHAOS set");
        return;
    }

    let (pool, _container, _url, _port) = chaos_common::boot_isolated_chaos_db().await;
    let queue = chaos_common::unique_queue();

    let config = WorkerConfig {
        concurrency: 1,
        poll_interval: Duration::from_millis(50),
        sweeper_interval: Duration::from_millis(200),
        shutdown_timeout: Duration::from_secs(5),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        lease_duration: Duration::from_secs(5),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<FailingTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build engine");

    let saved = engine
        .enqueue_raw(
            &queue,
            "failing_task",
            serde_json::json!({"message": "intentional failure"}),
            None,
            None,
            Some(MAX_ATTEMPTS),
            None,
            None,
        )
        .await
        .expect("enqueue");
    let task_id = saved.id();

    let token = CancellationToken::new();
    let engine = Arc::new(engine);
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let engine_handle = tokio::spawn(async move {
        if let Err(e) = engine_bg.start(token_bg).await {
            eprintln!("[engine] exited with error: {e}");
        }
    });

    // Wait for the task to exhaust all retries.
    let reached_failed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let task = engine.find(task_id).await.expect("find").expect("exists");
            if task.status() == TaskStatus::Failed {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("task should reach Failed within 30s");

    assert_eq!(reached_failed.status(), TaskStatus::Failed);
    assert_eq!(
        reached_failed.attempts().get(),
        MAX_ATTEMPTS,
        "should have exhausted all {MAX_ATTEMPTS} attempts"
    );
    assert!(
        reached_failed.last_error().is_some(),
        "last_error should be set"
    );

    // Wait an additional sweep cycle — assert task stays Failed.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let still_failed = engine.find(task_id).await.expect("find").expect("exists");
    assert_eq!(
        still_failed.status(),
        TaskStatus::Failed,
        "task must remain Failed after additional sweep cycle"
    );

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE id = $1 AND status = 'pending'")
            .bind(task_id.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("count pending");
    assert_eq!(
        pending, 0,
        "exhausted task must never be re-queued to pending"
    );

    token.cancel();
    tokio::time::timeout(Duration::from_secs(10), engine_handle)
        .await
        .expect("engine should exit")
        .expect("engine should not panic");
}
