mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskId, TaskRecord, TaskStatus,
    WorkerConfig,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Task that suspends on first execution and completes on second (when signal_payload is present).
#[derive(Debug, Serialize, Deserialize)]
struct SuspendableTask {}

impl Task for SuspendableTask {
    const KIND: &'static str = "suspendable";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        if ctx.signal_payload().is_some() {
            return Ok(());
        }
        ctx.suspend(None).await
    }
}

/// Task that suspends with checkpoint data.
#[derive(Debug, Serialize, Deserialize)]
struct SuspendWithCheckpointTask {}

impl Task for SuspendWithCheckpointTask {
    const KIND: &'static str = "suspend_checkpoint";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        if ctx.signal_payload().is_some() {
            return Ok(());
        }
        ctx.suspend(Some(serde_json::json!({"step": 3}))).await
    }
}

/// Task that just completes immediately (used as a second task for concurrency tests).
#[derive(Debug, Serialize, Deserialize)]
struct QuickTask {}

impl Task for QuickTask {
    const KIND: &'static str = "quick";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

fn fast_worker_config() -> WorkerConfig {
    WorkerConfig {
        concurrency: 4,
        poll_interval: Duration::from_millis(50),
        lease_duration: Duration::from_secs(60),
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(60),
        log_payload: false,
        sweeper_interval: Duration::from_millis(200),
        max_claim_backoff: Duration::from_secs(5),
        shutdown_timeout: Duration::from_secs(5),
        idempotency_key_retention: Duration::from_secs(3600),
        suspend_timeout: Duration::from_secs(24 * 60 * 60),
        region: None,
    }
}

async fn build_engine(pool: sqlx::PgPool, queue: &str) -> Arc<IronDefer> {
    Arc::new(
        IronDefer::builder()
            .pool(pool)
            .register::<SuspendableTask>()
            .register::<SuspendWithCheckpointTask>()
            .register::<QuickTask>()
            .worker_config(fast_worker_config())
            .queue(queue)
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine"),
    )
}

async fn build_engine_with_config(
    pool: sqlx::PgPool,
    queue: &str,
    config: WorkerConfig,
) -> Arc<IronDefer> {
    Arc::new(
        IronDefer::builder()
            .pool(pool)
            .register::<SuspendableTask>()
            .register::<SuspendWithCheckpointTask>()
            .register::<QuickTask>()
            .worker_config(config)
            .queue(queue)
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine"),
    )
}

/// Wait for a task to reach a target status, with timeout.
async fn wait_for_status(
    engine: &IronDefer,
    task_id: TaskId,
    target: TaskStatus,
    timeout: Duration,
) -> TaskRecord {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() > deadline {
            let record = engine.find(task_id).await.unwrap().unwrap();
            panic!(
                "task {} did not reach {:?} within {:?}, current status: {:?}",
                task_id,
                target,
                timeout,
                record.status()
            );
        }
        if let Some(record) = engine.find(task_id).await.unwrap() {
            if record.status() == target {
                return record;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// --- Tests ---

#[tokio::test]
async fn suspend_transitions_to_suspended() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool, &queue).await;
    let token = CancellationToken::new();

    let record = engine
        .enqueue(&queue, SuspendableTask {})
        .await
        .expect("enqueue");
    let task_id = record.id();

    let engine_run = engine.clone();
    let token_run = token.clone();
    tokio::spawn(async move { engine_run.start(token_run).await });

    let suspended = wait_for_status(&engine, task_id, TaskStatus::Suspended, Duration::from_secs(10)).await;
    token.cancel();

    assert_eq!(suspended.status(), TaskStatus::Suspended);
    assert!(suspended.suspended_at().is_some());
    assert!(suspended.claimed_by().is_none());
    assert!(suspended.claimed_until().is_none());
}

#[tokio::test]
async fn signal_resumes_suspended_task() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool, &queue).await;
    let token = CancellationToken::new();

    let record = engine
        .enqueue(&queue, SuspendableTask {})
        .await
        .expect("enqueue");
    let task_id = record.id();

    let engine_run = engine.clone();
    let token_run = token.clone();
    tokio::spawn(async move { engine_run.start(token_run).await });

    wait_for_status(&engine, task_id, TaskStatus::Suspended, Duration::from_secs(10)).await;

    let signal_payload = serde_json::json!({"approved": true, "reviewer": "alice"});
    let signaled = engine
        .signal(task_id, Some(signal_payload.clone()))
        .await
        .expect("signal");
    assert_eq!(signaled.status(), TaskStatus::Pending);
    assert!(signaled.suspended_at().is_none());
    assert_eq!(signaled.signal_payload(), Some(&signal_payload));

    let completed = wait_for_status(&engine, task_id, TaskStatus::Completed, Duration::from_secs(10)).await;
    token.cancel();

    assert_eq!(completed.status(), TaskStatus::Completed);
}

#[tokio::test]
async fn concurrent_signals_exactly_one_wins() {
    let Some(url) = common::test_db_url().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(15)
        .connect(url)
        .await
        .expect("pool");

    let queue = common::unique_queue();
    let engine = build_engine(pool, &queue).await;
    let token = CancellationToken::new();

    let record = engine
        .enqueue(&queue, SuspendableTask {})
        .await
        .expect("enqueue");
    let task_id = record.id();

    let engine_run = engine.clone();
    let token_run = token.clone();
    tokio::spawn(async move { engine_run.start(token_run).await });

    wait_for_status(&engine, task_id, TaskStatus::Suspended, Duration::from_secs(10)).await;

    let barrier = Arc::new(tokio::sync::Barrier::new(10));
    let mut handles = Vec::new();

    for i in 0..10 {
        let engine_clone = engine.clone();
        let barrier_clone = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier_clone.wait().await;
            engine_clone
                .signal(task_id, Some(serde_json::json!({"attempt": i})))
                .await
        }));
    }

    let mut success_count = 0u32;
    let mut conflict_count = 0u32;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(_) => success_count += 1,
            Err(TaskError::NotInExpectedState { .. }) => conflict_count += 1,
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    token.cancel();

    assert_eq!(success_count, 1, "exactly one signal should succeed");
    assert_eq!(conflict_count, 9, "nine signals should get 409");
}

#[tokio::test]
async fn suspend_watchdog_auto_fails() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let mut config = fast_worker_config();
    config.suspend_timeout = Duration::from_secs(1);
    config.sweeper_interval = Duration::from_millis(200);

    let engine = build_engine_with_config(pool, &queue, config).await;
    let token = CancellationToken::new();

    let record = engine
        .enqueue(&queue, SuspendableTask {})
        .await
        .expect("enqueue");
    let task_id = record.id();

    let engine_run = engine.clone();
    let token_run = token.clone();
    tokio::spawn(async move { engine_run.start(token_run).await });

    wait_for_status(&engine, task_id, TaskStatus::Suspended, Duration::from_secs(10)).await;

    let failed = wait_for_status(&engine, task_id, TaskStatus::Failed, Duration::from_secs(15)).await;
    token.cancel();

    assert_eq!(failed.status(), TaskStatus::Failed);
    assert_eq!(
        failed.last_error(),
        Some("suspend timeout exceeded")
    );
}

#[tokio::test]
async fn suspended_task_not_counted_in_concurrency() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let mut config = fast_worker_config();
    config.concurrency = 1;

    let engine = build_engine_with_config(pool, &queue, config).await;
    let token = CancellationToken::new();

    let suspend_record = engine
        .enqueue(&queue, SuspendableTask {})
        .await
        .expect("enqueue suspendable");
    let suspend_id = suspend_record.id();

    let engine_run = engine.clone();
    let token_run = token.clone();
    tokio::spawn(async move { engine_run.start(token_run).await });

    wait_for_status(&engine, suspend_id, TaskStatus::Suspended, Duration::from_secs(10)).await;

    let quick_record = engine
        .enqueue(&queue, QuickTask {})
        .await
        .expect("enqueue quick");
    let quick_id = quick_record.id();

    let completed = wait_for_status(&engine, quick_id, TaskStatus::Completed, Duration::from_secs(10)).await;
    token.cancel();

    assert_eq!(completed.status(), TaskStatus::Completed);
}

#[tokio::test]
async fn signal_on_non_suspended_returns_409() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool, &queue).await;

    let record = engine
        .enqueue(&queue, QuickTask {})
        .await
        .expect("enqueue");

    let result = engine.signal(record.id(), Some(serde_json::json!({}))).await;
    match result {
        Err(TaskError::NotInExpectedState { .. }) => {}
        other => panic!("expected NotInExpectedState, got {other:?}"),
    }
}

#[tokio::test]
async fn signal_on_nonexistent_returns_404() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool, &queue).await;

    let fake_id = TaskId::from_uuid(Uuid::new_v4());
    let result = engine.signal(fake_id, None).await;
    match result {
        Err(TaskError::NotFound { .. }) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}
