//! P1-INT-005 — `zombie_recoveries_total` counter increments when the
//! sweeper recovers a zombie task with an expired lease.
//!
//! Isolated in its own binary because `recover_zombie_tasks()` operates
//! globally (all queues). Running alongside other sweeper tests causes
//! the non-instrumented standalone sweeper to recover the zombie before
//! the engine's instrumented sweeper, suppressing the counter emission.

mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{IronDefer, QueueName, Task, TaskContext, TaskError, TaskStatus, WorkerConfig};
use iron_defer_application::TaskRepository;
use iron_defer_domain::WorkerId;
use iron_defer_infrastructure::PostgresTaskRepository;
use serde::{Deserialize, Serialize};

use common::otel::{build_harness, find_sample, scrape_samples, with_worker};

// ---------------------------------------------------------------------------
// Task fixture.
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
// Test.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sweeper_increments_zombie_recovery_counter() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();
    let harness = build_harness();

    let config = WorkerConfig {
        poll_interval: Duration::from_millis(50),
        sweeper_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(1),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SuccessTask>()
        .worker_config(config)
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .skip_migrations(true)
        .build()
        .await
        .expect("build");

    let saved = engine
        .enqueue(&queue, SuccessTask { n: 1 })
        .await
        .expect("enqueue");
    let task_id = saved.id();

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), false)) as Arc<dyn TaskRepository>;
    let queue_name = QueueName::try_from(queue.as_str()).expect("valid queue");
    let worker_id = WorkerId::new();
    let claimed = repo
        .claim_next(&queue_name, worker_id, Duration::from_millis(100), None)
        .await
        .expect("claim");
    assert!(claimed.is_some(), "should claim the task");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let engine = Arc::new(engine);
    with_worker(engine.clone(), |engine, _token| async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                tokio::time::Instant::now() <= deadline,
                "timed out waiting for recovered task to complete"
            );
            let found = engine.find(task_id).await.expect("find").expect("task");
            if found.status() == TaskStatus::Completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    let queue_str = QueueName::try_from(queue.as_str())
        .expect("valid queue")
        .to_string();
    let samples = scrape_samples(&harness.registry);
    let recovery_counter = find_sample(
        &samples,
        "iron_defer_zombie_recoveries_total_total",
        &[("queue", &queue_str)],
    );
    assert!(
        recovery_counter.is_some(),
        "expected iron_defer_zombie_recoveries_total_total with queue={queue_str}, \
         available metrics: {:?}",
        samples
            .iter()
            .map(|s| (&s.metric, &s.labels))
            .collect::<Vec<_>>()
    );
    assert!(
        recovery_counter.unwrap().value >= 1.0,
        "expected zombie recovery counter >= 1, got {}",
        recovery_counter.unwrap().value
    );

    harness.provider.shutdown().expect("provider shutdown");
}
