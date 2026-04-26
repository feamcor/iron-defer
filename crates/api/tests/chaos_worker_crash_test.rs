//! Chaos test: Worker crash recovery (AC 1, TEA P1-CHAOS-001).
//!
//! A "crashed" worker is simulated by claiming tasks directly via
//! `PostgresTaskRepository::claim_next()` with a short lease and never
//! completing them. The sweeper recovers the orphaned tasks, and a real
//! worker pool completes them — zero task loss.

mod chaos_common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    CancellationToken, IronDefer, QueueName, Task, TaskContext, TaskError, TaskStatus, WorkerConfig,
};
use iron_defer_domain::WorkerId;
use iron_defer_infrastructure::PostgresTaskRepository;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuccessTask {
    n: usize,
}

impl Task for SuccessTask {
    const KIND: &'static str = "success_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

const TASK_COUNT: usize = 10;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn worker_crash_recovery_zero_task_loss() {
    if chaos_common::should_skip() {
        eprintln!("[skip] IRON_DEFER_SKIP_DOCKER_CHAOS set");
        return;
    }

    let (pool, _container, _url, _port) = chaos_common::boot_isolated_chaos_db().await;
    let queue = chaos_common::unique_queue();

    let lease_duration = Duration::from_secs(2);

    let config = WorkerConfig {
        concurrency: 4,
        poll_interval: Duration::from_millis(50),
        sweeper_interval: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(10),
        lease_duration,
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SuccessTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build engine");

    for i in 0..TASK_COUNT {
        engine
            .enqueue(&queue, SuccessTask { n: i })
            .await
            .expect("enqueue");
    }

    // Simulate a "crashed worker": claim all tasks directly and never complete them.
    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), false))
        as Arc<dyn iron_defer_application::TaskRepository>;
    let queue_name = QueueName::try_from(queue.as_str()).expect("valid queue");
    let fake_worker = WorkerId::new();

    let mut claimed_count = 0;
    for _ in 0..TASK_COUNT {
        if let Some(_task) = repo
            .claim_next(&queue_name, fake_worker, lease_duration, None)
            .await
            .expect("claim")
        {
            claimed_count += 1;
        }
    }
    assert_eq!(
        claimed_count, TASK_COUNT,
        "should have claimed all {TASK_COUNT} tasks"
    );

    // Wait for leases to expire.
    tokio::time::sleep(lease_duration + Duration::from_millis(500)).await;

    // Start the real worker pool (with sweeper) to recover and complete orphaned tasks.
    let token = CancellationToken::new();
    let engine = Arc::new(engine);
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let engine_handle = tokio::spawn(async move {
        if let Err(e) = engine_bg.start(token_bg).await {
            eprintln!("[engine] exited with error: {e}");
        }
    });

    // Wait for all tasks to be recovered and completed.
    let completion = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let mut all_done = true;
            let tasks = engine.list(&queue).await.expect("list tasks");
            for task in &tasks {
                if task.status() != TaskStatus::Completed {
                    all_done = false;
                    break;
                }
            }
            if all_done && tasks.len() == TASK_COUNT {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;

    token.cancel();
    let join = tokio::time::timeout(Duration::from_secs(10), engine_handle).await;

    assert!(
        completion.is_ok(),
        "tasks did not all complete within 30s after crash recovery"
    );

    match join {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("engine task panicked: {e}"),
        Err(e) => panic!("engine did not exit within 10s after cancellation: {e}"),
    }

    // Final verification via SQL.
    let completed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count completed");
    assert_eq!(
        completed,
        i64::try_from(TASK_COUNT).expect("fits"),
        "all {TASK_COUNT} tasks must be completed"
    );

    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'running'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count running");
    assert_eq!(running, 0, "zero tasks should be in running state");

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'pending'")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count pending");
    assert_eq!(pending, 0, "zero tasks should be in pending state");
}
