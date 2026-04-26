//! Story 6.2 AC 4 — Dedicated lifecycle log field test.
//!
//! Replaces the coverage lost when `db_outage_integration_test.rs` was
//! renamed to `chaos_db_outage_test.rs` without its log assertions
//! (Story 3.1 AC 7). Verifies that structured log output during task
//! lifecycle transitions contains the required tracing fields:
//! `task_id`, `queue`, `worker_id`, and `attempt`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

use common::otel::{await_all_terminal, with_worker};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimpleTask {
    tag: String,
}

impl Task for SimpleTask {
    const KIND: &'static str = "lifecycle_log_simple";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[tokio::test]
#[tracing_test::traced_test]
async fn lifecycle_fields_task_id_queue_worker_id_attempt() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable — skipping lifecycle log field test");
        return;
    };

    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SimpleTask>()
        .queue(&queue)
        .skip_migrations(true)
        .worker_config(WorkerConfig {
            concurrency: 1,
            shutdown_timeout: Duration::from_secs(1),
            ..WorkerConfig::default()
        })
        .build()
        .await
        .expect("build engine");
    let engine = Arc::new(engine);

    let record = engine
        .enqueue(
            &queue,
            SimpleTask {
                tag: "lifecycle".into(),
            },
        )
        .await
        .expect("enqueue task");

    let task_id_str = record.id().to_string();

    with_worker(engine.clone(), |engine, _token| {
        let queue = queue.clone();
        async move {
            assert!(
                await_all_terminal(&engine, &queue, 50, Duration::from_millis(200)).await,
                "task did not reach terminal status in 10 s (see stderr for stuck-task diagnostic)"
            );
        }
    })
    .await;

    // task_enqueued: must contain task_id and queue.
    let enqueued_probe = format!("\"task_enqueued\" task_id={task_id_str}");
    assert!(
        logs_contain(&enqueued_probe),
        "expected `{enqueued_probe}` — task_enqueued missing task_id"
    );
    assert!(
        logs_contain(&format!("queue={queue}")),
        "task_enqueued missing queue field"
    );

    // task_claimed: must contain task_id, queue, worker_id, attempt.
    let claimed_probe = format!("\"task_claimed\" task_id={task_id_str}");
    assert!(
        logs_contain(&claimed_probe),
        "expected `{claimed_probe}` — task_claimed missing or task_id not emitted"
    );
    assert!(
        logs_contain("worker_id="),
        "task_claimed missing worker_id field"
    );
    assert!(
        logs_contain("attempt="),
        "task_claimed missing attempt field"
    );

    // task_completed: must contain task_id, queue, worker_id, attempt.
    let completed_probe = format!("\"task_completed\" task_id={task_id_str}");
    assert!(
        logs_contain(&completed_probe),
        "expected `{completed_probe}` — task_completed missing or task_id not emitted"
    );
}
