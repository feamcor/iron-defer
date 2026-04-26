//! Story 3.3 AC 8 — FR21 SQL audit trail compliance evidence.
//!
//! Standalone test binary (deliberately split from
//! `otel_compliance_test.rs`): a future compliance auditor reviewing
//! PCI DSS Req. 10 evidence should be able to read this file alone and
//! see the exact SQL queries that prove the requirement. If the `OTel`
//! test harness ever breaks (e.g. a major `opentelemetry-prometheus`
//! re-versioning), the SQL audit evidence remains unaffected.
//!
//! The test drives three tasks through distinct lifecycle paths:
//!
//! 1. `HAPPY` — enqueue → claim → complete.
//! 2. `TERMINAL` — enqueue → claim → fail-retry → claim → fail-terminal
//!    (same always-fails fixture as AC 6).
//! 3. `INTERRUPTED` — enqueued but never executed (worker is NOT
//!    started for this task's queue window), so the row stays in
//!    status `pending`. Dev Notes AC 8 option (c): abandon the third
//!    task rather than driving the shutdown-release path; the orphan
//!    is scoped by `queue` so it does not contaminate the other
//!    assertions.
//!
//! All six FR21 compliance SQL queries are then executed directly
//! against the test pool using `sqlx::query` / `sqlx::query_as`. The
//! runtime-typed path (not `sqlx::query!`) is intentional — it avoids
//! an `.sqlx/` offline-cache dependency that would otherwise require
//! `cargo sqlx prepare` before `cargo test` on a fresh checkout, and
//! the compliance evidence is equivalent: the same SQL shape executes.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use iron_defer::{IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Q4Row = (
    Uuid,
    String,
    i32,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditHappyTask {
    marker: u32,
}

impl Task for AuditHappyTask {
    const KIND: &'static str = "audit_happy_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditFlakyTask {}

impl Task for AuditFlakyTask {
    const KIND: &'static str = "audit_flaky_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Err(TaskError::ExecutionFailed {
            kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                reason: "synthetic-terminal".into(),
            },
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn sql_audit_trail_covers_fr21_compliance_queries() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };

    let queue = common::unique_queue();
    let test_start: DateTime<Utc> = Utc::now();

    // Build an engine that can claim + execute AuditHappyTask and
    // AuditFlakyTask. The interrupted third task (AuditHappyTask) is
    // enqueued but scheduled far enough in the future that the worker
    // never picks it up during this test window — see enqueue_at below.
    let worker_config = WorkerConfig {
        concurrency: 1,
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_secs(1),
        poll_interval: Duration::from_millis(50),
        shutdown_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<AuditHappyTask>()
        .register::<AuditFlakyTask>()
        .queue(&queue)
        .worker_config(worker_config)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    // Seed the three rows.
    let happy_record = engine
        .enqueue(&queue, AuditHappyTask { marker: 1 })
        .await
        .expect("enqueue happy");

    let terminal_record = engine
        .enqueue_raw(
            &queue,
            AuditFlakyTask::KIND,
            serde_json::to_value(AuditFlakyTask {}).expect("serialize"),
            None,
            None,
            Some(2),
            None,
            None,
        )
        .await
        .expect("enqueue_raw terminal");

    // Scheduled 1 hour ahead — the worker poll loop filters on
    // `scheduled_at <= now()`, so this row remains in `pending` for
    // the lifetime of the test. Dev Notes AC 8 option (c): an abandoned
    // row demonstrates the "claimed-but-not-completed" audit record
    // independently of shutdown choreography.
    let future_schedule = Utc::now() + chrono::Duration::hours(1);
    let interrupted_record = engine
        .enqueue_at(&queue, AuditHappyTask { marker: 2 }, future_schedule)
        .await
        .expect("enqueue_at interrupted");

    // Drive the worker until the two terminable tasks have settled.
    let token = CancellationToken::new();
    let worker_cancel = token.clone();
    let worker_engine = engine.clone();
    let worker_handle = tokio::spawn(async move {
        worker_engine
            .start(worker_cancel)
            .await
            .expect("engine start");
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "happy + terminal did not settle within 10 s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let happy = engine.find(happy_record.id()).await.expect("find happy");
        let term = engine
            .find(terminal_record.id())
            .await
            .expect("find terminal");
        if matches!(happy.map(|r| r.status()), Some(TaskStatus::Completed))
            && matches!(term.map(|r| r.status()), Some(TaskStatus::Failed))
        {
            break;
        }
    }

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_handle).await;

    // -------------------------------------------------------------------
    // FR21 compliance queries (AC 8 table). Each assertion prints the
    // observed rows on failure so an auditor can reproduce the mismatch.
    // -------------------------------------------------------------------

    let queue_str = queue.as_str();
    let happy_id: Uuid = *happy_record.id().as_uuid();
    let terminal_id: Uuid = *terminal_record.id().as_uuid();
    let interrupted_id: Uuid = *interrupted_record.id().as_uuid();

    // Q1: Who submitted task X and when?
    //     SELECT created_at, queue, kind, max_attempts FROM tasks WHERE id = $1
    let (created_at, q1_queue, q1_kind, q1_max_attempts): (DateTime<Utc>, String, String, i32) =
        sqlx::query_as("SELECT created_at, queue, kind, max_attempts FROM tasks WHERE id = $1")
            .bind(happy_id)
            .fetch_one(&pool)
            .await
            .expect("Q1: submitted fields");
    assert_eq!(q1_queue, queue_str, "Q1 queue mismatch");
    assert_eq!(q1_kind, "audit_happy_task", "Q1 kind mismatch");
    assert_eq!(q1_max_attempts, 3, "Q1 max_attempts mismatch (SQL default)");
    let skew = (created_at - test_start).num_seconds().abs();
    assert!(
        skew <= 10,
        "Q1 created_at should be within 10 s of test start, got {skew} s skew"
    );

    // Q2: Which worker claimed task X, when, for how long?
    //     Completed task: claimed_by IS NOT NULL (complete() does not clear).
    //     Failed-terminal task: claimed_by IS NOT NULL (last claim preserved).
    let happy_claim: (Option<Uuid>, Option<DateTime<Utc>>, i32) =
        sqlx::query_as("SELECT claimed_by, claimed_until, attempts FROM tasks WHERE id = $1")
            .bind(happy_id)
            .fetch_one(&pool)
            .await
            .expect("Q2a: happy claim fields");
    assert!(
        happy_claim.0.is_some(),
        "Q2a: completed task should retain last claimed_by (got {:?})",
        happy_claim.0
    );
    assert_eq!(happy_claim.2, 1, "Q2a: happy task attempts should = 1");

    let terminal_claim: (Option<Uuid>, Option<DateTime<Utc>>, i32) =
        sqlx::query_as("SELECT claimed_by, claimed_until, attempts FROM tasks WHERE id = $1")
            .bind(terminal_id)
            .fetch_one(&pool)
            .await
            .expect("Q2b: terminal claim fields");
    assert!(
        terminal_claim.0.is_some(),
        "Q2b: terminal-failed task should retain last claimed_by (got {:?})",
        terminal_claim.0
    );
    assert_eq!(
        terminal_claim.2, 2,
        "Q2b: terminal task attempts should = 2"
    );

    // Q3: How many attempts and why did task X ultimately fail?
    //     SELECT attempts, status, last_error FROM tasks WHERE id = $1
    let (q3_attempts, q3_status, q3_error): (i32, String, Option<String>) =
        sqlx::query_as("SELECT attempts, status::text, last_error FROM tasks WHERE id = $1")
            .bind(terminal_id)
            .fetch_one(&pool)
            .await
            .expect("Q3: terminal fail fields");
    assert_eq!(
        q3_attempts, 2,
        "Q3 attempts should = 2 (reached max_attempts)"
    );
    assert_eq!(q3_status, "failed", "Q3 status should be 'failed'");
    assert!(
        q3_error
            .as_ref()
            .is_some_and(|s| s.contains("synthetic-terminal")),
        "Q3 last_error should contain the synthetic reason, got {q3_error:?}"
    );

    // Q4: List all tasks in queue <q> over the last minute.
    //     SELECT id, status, attempts, last_error, created_at, updated_at
    //     FROM tasks
    //     WHERE queue = $1 AND created_at >= now() - interval '1 minute'
    //     ORDER BY created_at
    let q4_rows: Vec<Q4Row> = sqlx::query_as(
        "SELECT id, status::text, attempts, last_error, created_at, updated_at
             FROM tasks
             WHERE queue = $1 AND created_at >= now() - interval '1 minute'
             ORDER BY created_at",
    )
    .bind(queue_str)
    .fetch_all(&pool)
    .await
    .expect("Q4: recent tasks list");
    assert_eq!(
        q4_rows.len(),
        3,
        "Q4 should return three rows (happy, terminal, interrupted), got {}: {q4_rows:?}",
        q4_rows.len()
    );

    // Q5: Filter by status for compliance triage.
    //     SELECT id FROM tasks WHERE queue = $1 AND status = $2
    let q5_rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tasks WHERE queue = $1 AND status::text = $2")
            .bind(queue_str)
            .bind("failed")
            .fetch_all(&pool)
            .await
            .expect("Q5: failed status rows");
    assert_eq!(q5_rows.len(), 1, "Q5 should return the one failed task");
    assert_eq!(
        q5_rows[0].0, terminal_id,
        "Q5 id should match the terminal record"
    );

    // Q6: Time-range filter (DORA incident reconstruction pattern).
    //     SELECT id, status FROM tasks WHERE queue = $1
    //     AND updated_at BETWEEN $2 AND now()
    let q6_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status::text FROM tasks
         WHERE queue = $1 AND updated_at BETWEEN $2 AND now()",
    )
    .bind(queue_str)
    .bind(test_start)
    .fetch_all(&pool)
    .await
    .expect("Q6: time-range rows");
    assert_eq!(
        q6_rows.len(),
        3,
        "Q6 should return three rows in the test window, got {}: {q6_rows:?}",
        q6_rows.len()
    );

    // Defensive sanity check: the interrupted row is still `pending`.
    let (interrupted_status,): (String,) =
        sqlx::query_as("SELECT status::text FROM tasks WHERE id = $1")
            .bind(interrupted_id)
            .fetch_one(&pool)
            .await
            .expect("interrupted status");
    assert_eq!(
        interrupted_status, "pending",
        "interrupted task should remain 'pending' (scheduled_at > now); got {interrupted_status}"
    );
}
