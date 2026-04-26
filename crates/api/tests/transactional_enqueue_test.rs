//! Integration tests for transactional enqueue (Story 9.2).
//!
//! Tests cover:
//! - Committed transaction → task visible, eventually completed
//! - Rolled-back transaction → zero tasks, zero claims
//! - Idempotency key inside transaction → MVCC deduplication
//! - Concurrent worker poll during uncommitted window → zero tasks visible

mod common;

use std::sync::Arc;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TxTask {
    data: String,
}

impl Task for TxTask {
    const KIND: &'static str = "tx_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 9.2 AC 1 — Committed transaction → task visible to workers, completed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enqueue_in_committed_tx_visible_to_workers() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<TxTask>()
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);

    // Enqueue inside a transaction, then commit
    let mut tx = pool.begin().await.expect("begin tx");
    let record = engine
        .enqueue_in_tx(
            &mut tx,
            &queue,
            TxTask {
                data: "committed".into(),
            },
            None,
        )
        .await
        .expect("enqueue_in_tx");
    tx.commit().await.expect("commit");

    // Start workers AFTER commit so the task is definitely visible
    let token = CancellationToken::new();
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Poll until the task reaches completed
    let task_id = record.id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let found = engine
            .find(task_id)
            .await
            .expect("find")
            .expect("task must exist");
        if found.status() == TaskStatus::Completed {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "task {} did not reach completed within 10s, status: {:?}",
                task_id,
                found.status()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker_handle).await;
}

// ---------------------------------------------------------------------------
// 9.2 AC 2 — Rolled-back transaction → zero tasks, zero claims
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enqueue_in_rolled_back_tx_invisible() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<TxTask>()
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    // Enqueue inside a transaction, then rollback
    let mut tx = pool.begin().await.expect("begin tx");
    let _record = engine
        .enqueue_in_tx(
            &mut tx,
            &queue,
            TxTask {
                data: "rollback".into(),
            },
            None,
        )
        .await
        .expect("enqueue_in_tx");
    tx.rollback().await.expect("rollback");

    // Verify zero tasks exist for this queue
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE queue = $1")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 0, "rolled-back tx must leave zero tasks");

    // Brief wait + re-check to confirm no worker claimed during the window
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (count2,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE queue = $1")
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count2");
    assert_eq!(count2, 0, "zero tasks after brief wait (no phantom claims)");
}

// ---------------------------------------------------------------------------
// 9.2 AC 3 — Idempotency key inside transaction (MVCC isolation)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enqueue_idempotent_in_tx_mvcc_isolation() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<TxTask>()
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    // Transaction A: enqueue with idempotency key, commit
    let mut tx_a = pool.begin().await.expect("begin tx_a");
    let (record_a, created_a) = engine
        .enqueue_in_tx_idempotent(
            &mut tx_a,
            &queue,
            TxTask {
                data: "tx-a".into(),
            },
            &key,
            None,
        )
        .await
        .expect("enqueue_in_tx_idempotent A");
    assert!(created_a, "first insert must create");
    tx_a.commit().await.expect("commit tx_a");

    // Transaction B: same key, same queue → should see the existing task
    let mut tx_b = pool.begin().await.expect("begin tx_b");
    let (record_b, created_b) = engine
        .enqueue_in_tx_idempotent(
            &mut tx_b,
            &queue,
            TxTask {
                data: "tx-b".into(),
            },
            &key,
            None,
        )
        .await
        .expect("enqueue_in_tx_idempotent B");
    assert!(!created_b, "duplicate key must return existing");
    assert_eq!(
        record_a.id(),
        record_b.id(),
        "must return the same task record"
    );
    tx_b.commit().await.expect("commit tx_b");

    // Verify exactly 1 task in DB
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND idempotency_key = $2",
    )
    .bind(&queue)
    .bind(&key)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "exactly 1 task with this idempotency key");
}

// ---------------------------------------------------------------------------
// 9.2 AC 1+2 — Worker poll during uncommitted window sees zero tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worker_sees_zero_tasks_during_uncommitted_window() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<TxTask>()
        .queue(&queue)
        .skip_migrations(true)
        .worker_config({
            let mut wc = iron_defer::WorkerConfig::default();
            wc.poll_interval = std::time::Duration::from_millis(50);
            wc
        })
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);

    // Start workers BEFORE the enqueue — they will be polling the queue
    let token = CancellationToken::new();
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Allow workers to start polling
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Begin transaction, enqueue, but DON'T commit yet
    let mut tx = pool.begin().await.expect("begin tx");
    let record = engine
        .enqueue_in_tx(
            &mut tx,
            &queue,
            TxTask {
                data: "uncommitted".into(),
            },
            None,
        )
        .await
        .expect("enqueue_in_tx");

    // Workers are polling — verify no task is claimed from a SEPARATE connection
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Use a separate pool connection to check task visibility
    let (visible_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status IN ('running', 'completed')",
    )
    .bind(&queue)
    .fetch_one(&pool)
    .await
    .expect("visible count");
    assert_eq!(
        visible_count, 0,
        "uncommitted task must not be claimed by workers (SKIP LOCKED + READ COMMITTED)"
    );

    // Now commit — the task should become claimable
    tx.commit().await.expect("commit");

    // Wait for the task to be picked up and completed
    let task_id = record.id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let found = engine
            .find(task_id)
            .await
            .expect("find")
            .expect("task must exist after commit");
        if found.status() == TaskStatus::Completed {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "task {} did not reach completed within 10s after commit, status: {:?}",
                task_id,
                found.status()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker_handle).await;
}
