//! Worker pool integration tests (Story 1B.2, AC 8).
//!
//! Split from `worker_integration_test.rs` for maintainability.
//! Tests: end-to-end processing, bounded concurrency, retry behavior.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

use iron_defer::{
    CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Task types.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConcurrencyTask {
    n: i32,
}

/// RESET CONTRACT: `CONCURRENCY_ACTIVE` and `CONCURRENCY_PEAK` MUST be
/// reset to 0 before each test that uses `ConcurrencyTask`, otherwise
/// stale values from prior tests will corrupt peak-concurrency assertions.
static CONCURRENCY_ACTIVE: AtomicU32 = AtomicU32::new(0);
static CONCURRENCY_PEAK: AtomicU32 = AtomicU32::new(0);

impl Task for ConcurrencyTask {
    const KIND: &'static str = "concurrency_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        let current = CONCURRENCY_ACTIVE.fetch_add(1, Ordering::SeqCst) + 1;
        CONCURRENCY_PEAK.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        CONCURRENCY_ACTIVE.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryTask {
    n: i32,
}

/// RESET CONTRACT: `RETRY_ATTEMPT` MUST be reset to 0 before each test
/// that uses `RetryTask`, otherwise stale values will corrupt retry logic.
static RETRY_ATTEMPT: AtomicI32 = AtomicI32::new(0);

impl Task for RetryTask {
    const KIND: &'static str = "retry_task";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        let attempt = RETRY_ATTEMPT.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= 1 {
            Err(TaskError::ExecutionFailed {
                kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                    source: format!("deliberate failure on attempt {}", ctx.attempt().get()).into(),
                },
            })
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// **LOAD-BEARING TEST** — Submit 5 tasks, start the worker pool, wait for
/// all to reach Completed. Verify via raw SQL that the database state matches.
#[tokio::test]
async fn worker_processes_tasks_end_to_end() {
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

    let mut task_ids = Vec::new();
    for i in 0..5 {
        let saved = engine
            .enqueue(&queue, SuccessTask { n: i })
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
            "timed out waiting for tasks to complete"
        );

        let mut all_done = true;
        for &id in &task_ids {
            let found = engine_ref
                .find(id)
                .await
                .expect("find")
                .expect("task exists");
            if found.status() != TaskStatus::Completed {
                all_done = false;
                break;
            }
        }
        if all_done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for &id in &task_ids {
        let found = engine_ref
            .find(id)
            .await
            .expect("find")
            .expect("task exists");
        assert_eq!(found.status(), TaskStatus::Completed);
        assert_eq!(found.attempts().get(), 1);
    }

    let queue_str: String = queue.clone();
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM tasks WHERE queue = $1 AND status = 'completed'")
            .bind(&queue_str)
            .fetch_one(engine_ref.pool())
            .await
            .expect("raw SQL count");

    assert_eq!(count.0, 5);

    token.cancel();
    worker_handle.await.expect("join").expect("worker ok");
}

/// Submit 20 tasks with concurrency = 4. Verify peak concurrency never exceeds 4.
#[tokio::test]
async fn worker_bounded_concurrency_integration() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    CONCURRENCY_ACTIVE.store(0, Ordering::SeqCst);
    CONCURRENCY_PEAK.store(0, Ordering::SeqCst);

    let queue = common::unique_queue();
    let config = WorkerConfig {
        concurrency: 4,
        poll_interval: Duration::from_millis(20),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<ConcurrencyTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let mut task_ids = Vec::new();
    for i in 0..20 {
        let saved = engine
            .enqueue(&queue, ConcurrencyTask { n: i })
            .await
            .expect("enqueue");
        task_ids.push(saved.id());
    }

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            tokio::time::Instant::now() <= deadline,
            "timed out waiting for 20 tasks to complete"
        );
        let mut all_done = true;
        for &id in &task_ids {
            let found = engine_ref.find(id).await.expect("find").expect("task");
            if found.status() != TaskStatus::Completed {
                all_done = false;
                break;
            }
        }
        if all_done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for &id in &task_ids {
        let found = engine_ref.find(id).await.expect("find").expect("task");
        assert_eq!(found.status(), TaskStatus::Completed);
    }

    let peak = CONCURRENCY_PEAK.load(Ordering::SeqCst);
    assert!(peak <= 4, "peak concurrency {peak} exceeded limit 4");

    token.cancel();
    worker_handle.await.expect("join").expect("worker ok");
}

/// Register a handler that fails on first attempt, succeeds on second.
/// Verify it eventually completes with attempts=2.
#[tokio::test]
async fn worker_retries_failed_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    RETRY_ATTEMPT.store(0, Ordering::SeqCst);

    let queue = common::unique_queue();
    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(1),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<RetryTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build");

    let saved = engine
        .enqueue(&queue, RetryTask { n: 1 })
        .await
        .expect("enqueue");
    let task_id = saved.id();

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if tokio::time::Instant::now() > deadline {
            let found = engine_ref.find(task_id).await.expect("find").expect("task");
            panic!(
                "timed out waiting for retry task to complete. Current status: {:?}, attempts: {}",
                found.status(),
                found.attempts().get()
            );
        }
        let found = engine_ref.find(task_id).await.expect("find").expect("task");
        if found.status() == TaskStatus::Completed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let found = engine_ref.find(task_id).await.expect("find").expect("task");
    assert_eq!(found.status(), TaskStatus::Completed);
    assert_eq!(found.attempts().get(), 2);

    token.cancel();
    worker_handle.await.expect("join").expect("worker ok");
}

/// Submit a task with a payload that can't be deserialized by the registered
/// handler. The worker catches `serde::Error → TaskError::InvalidPayload`
/// and fails the task with the deserialization error message.
#[tokio::test]
async fn worker_fails_task_with_bad_payload() {
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
        .enqueue_raw(
            &queue,
            "success_task",
            serde_json::json!({"wrong_field": "not_an_int"}),
            None,
            None,
            Some(1),
            None,
            None,
        )
        .await
        .expect("enqueue_raw");
    let task_id = saved.id();

    let token = CancellationToken::new();
    let token_clone = token.clone();
    let engine_ref = Arc::new(engine);
    let engine_worker = engine_ref.clone();
    let worker_handle = tokio::spawn(async move { engine_worker.start(token_clone).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() <= deadline,
            "timed out waiting for bad-payload task to fail"
        );
        let found = engine_ref.find(task_id).await.expect("find").expect("task");
        if found.status() == TaskStatus::Failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let found = engine_ref.find(task_id).await.expect("find").expect("task");
    assert_eq!(found.status(), TaskStatus::Failed);
    assert!(
        found
            .last_error()
            .is_some_and(|e| e.contains("missing field")),
        "last_error should contain serde deserialization message, got: {:?}",
        found.last_error()
    );

    token.cancel();
    worker_handle.await.expect("join").expect("worker ok");
}
