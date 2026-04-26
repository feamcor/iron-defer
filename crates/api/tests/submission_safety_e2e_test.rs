//! E2E tests for submission safety (Story 9.3).
//!
//! Tests cover idempotency and transactional enqueue guarantees under
//! concurrent load. Many scenarios are also covered in `idempotency_test.rs`
//! (REST API) and `transactional_enqueue_test.rs` (library API); this file
//! adds the remaining E2E scenarios specified in Story 9.3.

mod common;

use std::sync::Arc;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SafetyTask {
    data: String,
}

impl Task for SafetyTask {
    const KIND: &'static str = "safety_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 1.1 — Barrier-synchronized concurrent submission (library API)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_idempotent_enqueue_via_library_api() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = Arc::new(
        IronDefer::builder()
            .pool(pool.clone())
            .register::<SafetyTask>()
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine"),
    );

    let key = uuid::Uuid::new_v4().to_string();
    let barrier = Arc::new(tokio::sync::Barrier::new(10));
    let mut handles = Vec::new();

    for i in 0..10 {
        let engine = Arc::clone(&engine);
        let barrier = Arc::clone(&barrier);
        let q = queue.clone();
        let k = key.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            engine
                .enqueue_idempotent(
                    &q,
                    SafetyTask {
                        data: format!("thread-{i}"),
                    },
                    &k,
                )
                .await
        }));
    }

    let mut task_ids = std::collections::HashSet::new();
    let mut errors = Vec::new();
    for h in handles {
        match h.await.expect("join") {
            Ok((record, _created)) => {
                task_ids.insert(record.id());
            }
            Err(e) => errors.push(e),
        }
    }

    assert!(errors.is_empty(), "expected 0 errors, got {errors:?}");
    assert_eq!(task_ids.len(), 1, "all 10 submissions must return the same task");

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND idempotency_key = $2")
            .bind(&queue)
            .bind(&key)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 1, "exactly 1 task in DB");
}

// ---------------------------------------------------------------------------
// 1.5 — Completed task with unexpired key: re-submit creates new task
//
// The partial unique index excludes terminal statuses, so a completed task's
// key is immediately reusable regardless of idempotency_expires_at.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completed_task_with_active_key_allows_new_submission() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    let engine = Arc::new(
        IronDefer::builder()
            .pool(pool.clone())
            .register::<SafetyTask>()
            .queue(&queue)
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine"),
    );

    // Submit first task with idempotency key
    let (first_record, created) = engine
        .enqueue_idempotent(
            &queue,
            SafetyTask {
                data: "first".into(),
            },
            &key,
        )
        .await
        .expect("first enqueue");
    assert!(created, "first submission must create");

    // Start workers so the task gets completed
    let token = CancellationToken::new();
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Wait for the task to complete
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let found = engine
            .find(first_record.id())
            .await
            .expect("find")
            .expect("task must exist");
        if found.status() == TaskStatus::Completed {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("task did not reach completed within 10s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker_handle).await;

    // Verify the key is still present (NOT cleaned by sweeper — not expired)
    let (key_present,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE id = $1 AND idempotency_key IS NOT NULL",
    )
    .bind(first_record.id().as_uuid())
    .fetch_one(&pool)
    .await
    .expect("key check");
    assert_eq!(key_present, 1, "key should still be present (not yet expired)");

    // Re-submit with the same key — should create a NEW task because the
    // partial index excludes completed tasks
    let (second_record, created2) = engine
        .enqueue_idempotent(
            &queue,
            SafetyTask {
                data: "second".into(),
            },
            &key,
        )
        .await
        .expect("second enqueue");
    assert!(
        created2,
        "re-submit after completion must create new task (partial index excludes terminal)"
    );
    assert_ne!(
        first_record.id(),
        second_record.id(),
        "new task must have a different ID"
    );

    // Verify 2 tasks with the same key exist (one completed, one pending)
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND idempotency_key = $2")
            .bind(&queue)
            .bind(&key)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 2, "both the completed and new tasks should exist");
}

// ---------------------------------------------------------------------------
// 2.3 variant — Multiple tasks in one transaction, concurrent workers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_tasks_in_tx_invisible_until_commit() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = Arc::new(
        IronDefer::builder()
            .pool(pool.clone())
            .register::<SafetyTask>()
            .queue(&queue)
            .skip_migrations(true)
            .worker_config({
                let mut wc = iron_defer::WorkerConfig::default();
                wc.poll_interval = std::time::Duration::from_millis(50);
                wc.concurrency = 4;
                wc
            })
            .build()
            .await
            .expect("build engine"),
    );

    // Start workers BEFORE enqueue
    let token = CancellationToken::new();
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Allow workers to start polling
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Enqueue 5 tasks inside one transaction
    let mut tx = pool.begin().await.expect("begin tx");
    let mut task_ids = Vec::new();
    for i in 0..5 {
        let record = engine
            .enqueue_in_tx(
                &mut tx,
                &queue,
                SafetyTask {
                    data: format!("batch-{i}"),
                },
                None,
            )
            .await
            .expect("enqueue_in_tx");
        task_ids.push(record.id());
    }

    // Workers are polling — verify 0 tasks claimed
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let (claimed,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status IN ('running', 'completed')",
    )
    .bind(&queue)
    .fetch_one(&pool)
    .await
    .expect("claimed count");
    assert_eq!(claimed, 0, "uncommitted tasks must not be claimed");

    // Commit — tasks become visible
    tx.commit().await.expect("commit");

    // Wait for all 5 to reach completed
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (completed,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'",
        )
        .bind(&queue)
        .fetch_one(&pool)
        .await
        .expect("completed count");
        if completed >= 5 {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("not all 5 tasks completed within 15s, completed: {completed}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker_handle).await;
}

// ---------------------------------------------------------------------------
// 2.4 — Transactional enqueue + non-transactional idempotency cross-path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tx_enqueue_then_non_tx_dedup_returns_existing() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SafetyTask>()
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    // Enqueue with idempotency key inside a transaction, then commit
    let mut tx = pool.begin().await.expect("begin tx");
    let (tx_record, tx_created) = engine
        .enqueue_in_tx_idempotent(
            &mut tx,
            &queue,
            SafetyTask {
                data: "via-tx".into(),
            },
            &key,
            None,
        )
        .await
        .expect("enqueue_in_tx_idempotent");
    assert!(tx_created, "first insert via tx must create");
    tx.commit().await.expect("commit");

    // Non-transactional re-submit with same key → should return existing
    let (dedup_record, dedup_created) = engine
        .enqueue_idempotent(
            &queue,
            SafetyTask {
                data: "via-non-tx".into(),
            },
            &key,
        )
        .await
        .expect("non-tx dedup");
    assert!(!dedup_created, "duplicate key must return existing");
    assert_eq!(
        tx_record.id(),
        dedup_record.id(),
        "must return the same task"
    );
}
