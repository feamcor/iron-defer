mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{IronDefer, Task, TaskContext, TaskError};
use iron_defer_application::TaskRepository;
use iron_defer_infrastructure::PostgresTaskRepository;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct AuditTestTask {
    value: i32,
}

impl Task for AuditTestTask {
    const KIND: &'static str = "audit_test";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

async fn build_engine(pool: sqlx::PgPool, audit_log: bool) -> IronDefer {
    IronDefer::builder()
        .pool(pool)
        .register::<AuditTestTask>()
        .skip_migrations(true)
        .database_config(iron_defer_application::DatabaseConfig {
            audit_log,
            ..Default::default()
        })
        .build()
        .await
        .expect("engine build failed")
}

#[tokio::test]
async fn audit_lifecycle_pending_running_completed() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 1 })
        .await
        .expect("enqueue");

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), true))
        as Arc<dyn TaskRepository>;

    let worker_id = iron_defer_domain::WorkerId::new();
    let claimed = repo
        .claim_next(
            &iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap(),
            worker_id,
            Duration::from_secs(300),
            None,
        )
        .await
        .expect("claim")
        .expect("task should be available");

    repo.complete(claimed.id()).await.expect("complete");

    let entries = repo.audit_log(record.id(), 100, 0).await.expect("audit_log");
    assert_eq!(entries.entries.len(), 3, "expected 3 audit entries, got {}", entries.entries.len());

    assert!(entries.entries[0].from_status().is_none());
    assert_eq!(entries.entries[0].to_status(), iron_defer_domain::TaskStatus::Pending);

    assert_eq!(entries.entries[1].from_status(), Some(iron_defer_domain::TaskStatus::Pending));
    assert_eq!(entries.entries[1].to_status(), iron_defer_domain::TaskStatus::Running);
    assert_eq!(*entries.entries[1].worker_id().unwrap().as_uuid(), *worker_id.as_uuid());

    assert_eq!(entries.entries[2].from_status(), Some(iron_defer_domain::TaskStatus::Running));
    assert_eq!(entries.entries[2].to_status(), iron_defer_domain::TaskStatus::Completed);
}

#[tokio::test]
async fn audit_lifecycle_with_retry() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 2 })
        .await
        .expect("enqueue");

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), true))
        as Arc<dyn TaskRepository>;

    let qn = iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap();
    let worker_id = iron_defer_domain::WorkerId::new();

    // Claim + fail (retry)
    let claimed = repo
        .claim_next(&qn, worker_id, Duration::from_secs(300), None)
        .await
        .expect("claim1")
        .expect("available");
    let failed = repo
        .fail(claimed.id(), "transient error", 1.0, 60.0)
        .await
        .expect("fail");
    assert_eq!(failed.status(), iron_defer_domain::TaskStatus::Pending);

    // Make task immediately available for re-claim
    sqlx::query("UPDATE tasks SET scheduled_at = now() WHERE id = $1")
        .bind(record.id().as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    // Re-claim + complete
    let reclaimed = repo
        .claim_next(&qn, worker_id, Duration::from_secs(300), None)
        .await
        .expect("claim2")
        .expect("available");
    repo.complete(reclaimed.id()).await.expect("complete");

    let entries = repo.audit_log(record.id(), 100, 0).await.expect("audit_log");
    assert_eq!(entries.entries.len(), 5, "expected 5 audit entries, got {}", entries.entries.len());

    assert!(entries.entries[0].from_status().is_none());
    assert_eq!(entries.entries[0].to_status(), iron_defer_domain::TaskStatus::Pending);

    assert_eq!(entries.entries[1].from_status(), Some(iron_defer_domain::TaskStatus::Pending));
    assert_eq!(entries.entries[1].to_status(), iron_defer_domain::TaskStatus::Running);

    assert_eq!(entries.entries[2].from_status(), Some(iron_defer_domain::TaskStatus::Running));
    assert_eq!(entries.entries[2].to_status(), iron_defer_domain::TaskStatus::Pending);
    assert!(entries.entries[2].metadata().is_some());

    assert_eq!(entries.entries[3].from_status(), Some(iron_defer_domain::TaskStatus::Pending));
    assert_eq!(entries.entries[3].to_status(), iron_defer_domain::TaskStatus::Running);

    assert_eq!(entries.entries[4].from_status(), Some(iron_defer_domain::TaskStatus::Running));
    assert_eq!(entries.entries[4].to_status(), iron_defer_domain::TaskStatus::Completed);
}

#[tokio::test]
async fn audit_immutability_update_rejected() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 3 })
        .await
        .expect("enqueue");

    let entries = engine.audit_log(record.id(), 100, 0).await.expect("audit_log");
    let audit_id = entries.entries[0].id();

    let err = sqlx::query("UPDATE task_audit_log SET to_status = 'hacked' WHERE id = $1")
        .bind(audit_id)
        .execute(&pool)
        .await;

    assert!(err.is_err(), "UPDATE on audit log should be rejected");
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("append-only"), "error should mention append-only: {msg}");
}

#[tokio::test]
async fn audit_immutability_delete_rejected() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 4 })
        .await
        .expect("enqueue");

    let entries = engine.audit_log(record.id(), 100, 0).await.expect("audit_log");
    let audit_id = entries.entries[0].id();

    let err = sqlx::query("DELETE FROM task_audit_log WHERE id = $1")
        .bind(audit_id)
        .execute(&pool)
        .await;

    assert!(err.is_err(), "DELETE on audit log should be rejected");
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("append-only"), "error should mention append-only: {msg}");
}

#[tokio::test]
async fn audit_cancel_produces_audit_row() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 5 })
        .await
        .expect("enqueue");

    engine.cancel(record.id()).await.expect("cancel");

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), true))
        as Arc<dyn TaskRepository>;

    let entries = repo.audit_log(record.id(), 100, 0).await.expect("audit_log");
    assert_eq!(entries.entries.len(), 2, "expected 2 audit entries (create + cancel)");

    assert!(entries.entries[0].from_status().is_none());
    assert_eq!(entries.entries[0].to_status(), iron_defer::TaskStatus::Pending);

    assert_eq!(entries.entries[1].from_status(), Some(iron_defer::TaskStatus::Pending));
    assert_eq!(entries.entries[1].to_status(), iron_defer::TaskStatus::Cancelled);
    assert!(entries.entries[1].worker_id().is_none());
}

#[tokio::test]
async fn audit_disabled_no_rows() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), false).await;

    let record = engine
        .enqueue(&queue, AuditTestTask { value: 6 })
        .await
        .expect("enqueue");

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), false))
        as Arc<dyn TaskRepository>;

    let qn = iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap();
    let worker_id = iron_defer_domain::WorkerId::new();

    let claimed = repo
        .claim_next(&qn, worker_id, Duration::from_secs(300), None)
        .await
        .expect("claim")
        .expect("available");
    repo.complete(claimed.id()).await.expect("complete");

    // Even if we query with audit=true, verify no rows were inserted
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_audit_log WHERE task_id = $1",
    )
    .bind(record.id().as_uuid())
    .fetch_one(&pool)
    .await
    .expect("count query");

    assert_eq!(count, 0, "no audit rows should exist when audit_log=false");
}

#[tokio::test]
async fn audit_trace_id_correlation() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let trace_id = "abcdef1234567890abcdef1234567890";
    let record = engine
        .enqueue_raw(
            &queue,
            "audit_test",
            serde_json::json!({"value": 7}),
            None,
            None,
            None,
            Some(trace_id),
            None,
        )
        .await
        .expect("enqueue_raw");

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), true))
        as Arc<dyn TaskRepository>;

    let qn = iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap();
    let worker_id = iron_defer_domain::WorkerId::new();

    let claimed = repo
        .claim_next(&qn, worker_id, Duration::from_secs(300), None)
        .await
        .expect("claim")
        .expect("available");
    repo.complete(claimed.id()).await.expect("complete");

    let entries = repo.audit_log(record.id(), 100, 0).await.expect("audit_log");
    assert_eq!(entries.entries.len(), 3);

    for entry in &entries.entries {
        assert_eq!(
            entry.trace_id(),
            Some(trace_id),
            "all audit entries should carry the trace_id"
        );
    }
}

#[tokio::test]
async fn audit_atomicity_crosscheck() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), true).await;

    let repo = Arc::new(PostgresTaskRepository::new(pool.clone(), true))
        as Arc<dyn TaskRepository>;

    let qn = iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap();
    let worker_id = iron_defer_domain::WorkerId::new();

    // Submit 3 tasks, complete all
    let mut task_ids = Vec::new();
    for i in 0..3 {
        let record = engine
            .enqueue(&queue, AuditTestTask { value: i })
            .await
            .expect("enqueue");
        task_ids.push(record.id());
    }

    for _ in 0..3 {
        let claimed = repo
            .claim_next(&qn, worker_id, Duration::from_secs(300), None)
            .await
            .expect("claim")
            .expect("available");
        repo.complete(claimed.id()).await.expect("complete");
    }

    // Cross-check: every non-pending task must have audit entries
    for task_id in &task_ids {
        let entries = repo.audit_log(*task_id, 100, 0).await.expect("audit_log");
        assert_eq!(
            entries.entries.len(), 3,
            "task {} should have exactly 3 audit entries (create+claim+complete), got {}",
            task_id,
            entries.entries.len()
        );
    }
}
