//! End-to-end integration tests for the `IronDefer` library API
//! (Story 1A.3, AC 9).
//!
//! Most tests use `common::fresh_pool_on_shared_container()` to get a
//! per-test pool on the shared testcontainers Postgres instance. This
//! prevents cross-test `PoolTimedOut` from runtime-drop connection
//! stranding (see `common/mod.rs` doc comment). Tests that explicitly
//! need an unmigrated database use `common::fresh_unmigrated_pool`.
//!
//! Each test scopes its writes with `common::unique_queue()` so concurrent
//! tests in the same binary do not collide.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use iron_defer::{IronDefer, QueueName, Task, TaskContext, TaskError, TaskId};
use serde::{Deserialize, Serialize};

/// Sample task type used by every integration test in this module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EchoTask {
    message: String,
}

impl Task for EchoTask {
    const KIND: &'static str = "echo";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        // Story 1A.3 doesn't run executors — Epic 1B owns the worker pool.
        Ok(())
    }
}

#[tokio::test]
async fn builder_requires_pool() {
    // Cold path — no testcontainer needed.
    let result = IronDefer::builder().build().await;
    let err = result.expect_err("build without pool must error");
    assert!(
        matches!(err, TaskError::Storage { .. }),
        "expected TaskError::Storage variant, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("PgPool not provided"),
        "expected pool-missing error, got: {msg}"
    );
}

#[tokio::test]
async fn builder_runs_migrations_by_default() {
    // Use a FRESH unmigrated container so this test can actually prove
    // build() ran the migrator. With the shared TEST_DB pool the table
    // already exists from `boot_test_db` and we cannot distinguish
    // "build migrated" from "build skipped".
    let Some((pool, _container)) = common::fresh_unmigrated_pool().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Pre-condition: the tasks table does NOT exist on a fresh container.
    let pre: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT count(*) FROM tasks")
        .fetch_one(&pool)
        .await;
    assert!(
        pre.is_err(),
        "fresh container should not have tasks table before build()"
    );

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build succeeds with pool");

    // Post-condition: the tasks table now exists, proving the builder
    // ran the migrator.
    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM tasks")
        .fetch_one(engine.pool())
        .await
        .expect("tasks table must exist after default build");
}

#[tokio::test]
async fn builder_skip_migrations_does_not_run_migrator() {
    let Some((pool, _container)) = common::fresh_unmigrated_pool().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Build with skip_migrations(true) — must NOT create the tasks table.
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .build()
        .await
        .expect("build succeeds even with skip_migrations");

    // Confirm the tasks table does NOT exist via SQLSTATE 42P01
    // (`undefined_table`) — much tighter than substring matching, which
    // could spuriously pass on connection / auth / syntax errors.
    let result: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT count(*) FROM tasks")
        .fetch_one(engine.pool())
        .await;
    let err = result.expect_err("tasks table should not exist after skip_migrations build");
    let sqlstate = err
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .map(std::borrow::Cow::into_owned);
    assert_eq!(
        sqlstate.as_deref(),
        Some("42P01"),
        "expected SQLSTATE 42P01 (undefined_table), got: {err:?}"
    );
}

#[tokio::test]
async fn enqueue_persists_task_with_kind_and_default_scheduled_at() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let queue = common::unique_queue();
    let task = EchoTask {
        message: "hello world".into(),
    };
    let before = Utc::now();
    let saved = engine
        .enqueue(&queue, task.clone())
        .await
        .expect("enqueue succeeds");
    let after = Utc::now();

    assert_eq!(saved.kind(), EchoTask::KIND);
    assert_eq!(saved.queue().as_str(), queue);
    assert_eq!(saved.status(), iron_defer::TaskStatus::Pending);
    assert_eq!(saved.attempts().get(), 0);

    // Default scheduled_at is "now" — verify within a 5s tolerance.
    let tolerance = ChronoDuration::seconds(5);
    assert!(
        saved.scheduled_at() >= before - tolerance && saved.scheduled_at() <= after + tolerance,
        "scheduled_at {} should default to now ({}–{})",
        saved.scheduled_at(),
        before,
        after
    );

    // Payload round-trips back into the same EchoTask.
    let round_tripped: EchoTask =
        serde_json::from_value(saved.payload().clone()).expect("deserialize");
    assert_eq!(round_tripped, task);
}

#[tokio::test]
async fn enqueue_at_respects_explicit_schedule() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let scheduled = Utc::now() + ChronoDuration::hours(1);
    let queue = common::unique_queue();
    let saved = engine
        .enqueue_at(
            &queue,
            EchoTask {
                message: "later".into(),
            },
            scheduled,
        )
        .await
        .expect("enqueue_at succeeds");

    // Sub-millisecond drift expected from microsecond rounding in Postgres.
    let drift = (saved.scheduled_at() - scheduled).num_milliseconds().abs();
    assert!(drift < 5, "scheduled_at drift = {drift}ms");
}

#[tokio::test]
async fn find_returns_full_record() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let queue = common::unique_queue();
    let saved = engine
        .enqueue(
            &queue,
            EchoTask {
                message: "find-me".into(),
            },
        )
        .await
        .expect("enqueue");

    let fetched = engine
        .find(saved.id())
        .await
        .expect("find")
        .expect("task exists");

    assert_eq!(fetched, saved);
}

#[tokio::test]
async fn find_returns_none_for_missing_id() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let result = engine
        .find(TaskId::new())
        .await
        .expect("find succeeds for absent id");
    assert!(result.is_none());
}

#[tokio::test]
async fn list_returns_only_matching_queue() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let payments = common::unique_queue();
    let notifications = common::unique_queue();

    for i in 0..3 {
        engine
            .enqueue(
                &payments,
                EchoTask {
                    message: format!("pay-{i}"),
                },
            )
            .await
            .expect("enqueue payments");
    }
    for i in 0..2 {
        engine
            .enqueue(
                &notifications,
                EchoTask {
                    message: format!("notify-{i}"),
                },
            )
            .await
            .expect("enqueue notifications");
    }

    let payments_list = engine.list(&payments).await.expect("list payments");
    let notifications_list = engine
        .list(&notifications)
        .await
        .expect("list notifications");

    assert_eq!(payments_list.len(), 3);
    assert_eq!(notifications_list.len(), 2);
    assert!(payments_list.iter().all(|t| t.queue().as_str() == payments));
    assert!(
        notifications_list
            .iter()
            .all(|t| t.queue().as_str() == notifications)
    );
}

#[tokio::test]
async fn enqueue_rejects_invalid_queue_name() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<EchoTask>()
        .build()
        .await
        .expect("build");

    let err = engine
        .enqueue(
            "",
            EchoTask {
                message: "x".into(),
            },
        )
        .await
        .expect_err("empty queue name must be rejected");

    assert!(
        matches!(err, TaskError::InvalidPayload { .. }),
        "expected InvalidPayload variant, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("invalid queue name"),
        "expected static prefix in error, got: {msg}"
    );
    // The underlying ValidationError text must be visible per AC 9 — the
    // EmptyQueueName variant displays as "queue name must not be empty".
    assert!(
        msg.contains("must not be empty"),
        "expected underlying ValidationError text in error, got: {msg}"
    );
}

// Reference QueueName so an unused-import warning never appears in this
// test binary even when the type is touched only via the engine surface.
#[allow(dead_code)]
fn _queue_name_in_scope() -> QueueName {
    QueueName::try_from("anchor").expect("anchor")
}
