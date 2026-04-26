//! Story 3.3 AC 7 — lifecycle log records cover all transitions.
//!
//! Split from `otel_compliance_test.rs` for maintainability.
//! Structured log evidence for every FR19 transition via
//! `#[tracing_test::traced_test]`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use iron_defer::{IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

use common::otel::{await_all_terminal, with_worker};

// ---------------------------------------------------------------------------
// Task fixtures.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HappyTask {
    marker: u32,
}

impl Task for HappyTask {
    const KIND: &'static str = "otel_happy_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryOnceTask {
    marker: u32,
}

impl Task for RetryOnceTask {
    const KIND: &'static str = "otel_retry_once_task";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        RETRY_ONCE_EXECUTE_CALLS.fetch_add(1, Ordering::Relaxed);
        if ctx.attempt().get() == 1 {
            Err(TaskError::ExecutionFailed {
                kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                    source: "first-attempt-fail".into(),
                },
            })
        } else {
            Ok(())
        }
    }
}

/// RESET CONTRACT: this counter MUST be reset to 0 before each test that
/// uses `RetryOnceTask`, otherwise stale values from prior tests (within
/// the same binary) will corrupt the assertion.
static RETRY_ONCE_EXECUTE_CALLS: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// AC 7.
// ---------------------------------------------------------------------------

#[tokio::test]
#[tracing_test::traced_test]
#[allow(clippy::too_many_lines)]
async fn lifecycle_log_records_cover_all_transitions() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    RETRY_ONCE_EXECUTE_CALLS.store(0, Ordering::Relaxed);

    let queue = common::unique_queue();
    let worker_config = WorkerConfig {
        concurrency: 1,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(1),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<HappyTask>()
        .register::<RetryOnceTask>()
        .queue(&queue)
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    let happy_record = engine
        .enqueue(&queue, HappyTask { marker: 1 })
        .await
        .expect("enqueue happy");
    let retry_record = engine
        .enqueue_raw(
            &queue,
            RetryOnceTask::KIND,
            serde_json::to_value(RetryOnceTask { marker: 2 }).expect("serialize"),
            None,
            None,
            Some(2),
            None,
            None,
        )
        .await
        .expect("enqueue_raw retry-once");

    with_worker(engine.clone(), |engine, _token| {
        let queue = queue.clone();
        async move {
            assert!(
                await_all_terminal(&engine, &queue, 50, Duration::from_millis(200)).await,
                "AC 7 tasks did not all reach terminal status in 10 s (see stderr for stuck-task diagnostic)"
            );
        }
    })
    .await;

    let retry_calls = RETRY_ONCE_EXECUTE_CALLS.load(Ordering::Relaxed);
    assert_eq!(
        retry_calls, 2,
        "RetryOnceTask::execute should have run exactly twice (attempt 1 fail, attempt 2 success); got {retry_calls}"
    );

    let happy_id_str = happy_record.id().to_string();
    let retry_id_str = retry_record.id().to_string();

    for event in ["task_enqueued", "task_claimed", "task_completed"] {
        let probe = format!("\"{event}\" task_id={happy_id_str}");
        assert!(
            logs_contain(&probe),
            "expected `{probe}` in captured log stream (HappyTask lifecycle)"
        );
    }
    for event in [
        "task_enqueued",
        "task_claimed",
        "task_failed_retry",
        "task_completed",
    ] {
        let probe = format!("\"{event}\" task_id={retry_id_str}");
        assert!(
            logs_contain(&probe),
            "expected `{probe}` in captured log stream (RetryOnceTask lifecycle)"
        );
    }

    assert!(
        logs_contain(&queue),
        "queue name `{queue}` missing from log stream"
    );
    assert!(
        logs_contain("otel_happy_task"),
        "kind=otel_happy_task missing from log stream"
    );
    assert!(
        logs_contain("otel_retry_once_task"),
        "kind=otel_retry_once_task missing from log stream"
    );

    assert!(
        !logs_contain("payload="),
        "payload= leaked into default-privacy log stream (FR38 regression)"
    );
}

// ---------------------------------------------------------------------------
// task_failed_terminal log regression guard (#43 from deferred-work).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AlwaysFailTask {}

impl Task for AlwaysFailTask {
    const KIND: &'static str = "otel_always_fail_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Err(TaskError::ExecutionFailed {
            kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                source: "always-fails".into(),
            },
        })
    }
}

#[tokio::test]
#[tracing_test::traced_test]
async fn task_failed_terminal_log_event_emitted() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let worker_config = WorkerConfig {
        concurrency: 1,
        base_delay: Duration::from_millis(50),
        max_delay: Duration::from_secs(1),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<AlwaysFailTask>()
        .queue(&queue)
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    let record = engine
        .enqueue_raw(
            &queue,
            AlwaysFailTask::KIND,
            serde_json::to_value(AlwaysFailTask {}).expect("serialize"),
            None,
            None,
            Some(1),
            None,
            None,
        )
        .await
        .expect("enqueue always-fail task with max_attempts=1");

    with_worker(engine.clone(), |engine, _token| {
        let record_id = record.id();
        async move {
            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if let Some(r) = engine.find(record_id).await.expect("find task")
                    && matches!(r.status(), iron_defer::TaskStatus::Failed)
                {
                    return;
                }
            }
            panic!("always-fail task never reached Failed in 10 s");
        }
    })
    .await;

    let id_str = record.id().to_string();
    let probe = format!("\"task_failed_terminal\" task_id={id_str}");
    assert!(
        logs_contain(&probe),
        "expected `{probe}` in log stream — task_failed_terminal event missing for SOC 2 CC7.2"
    );
}
