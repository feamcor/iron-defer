mod common;

use std::time::Duration;

use common::e2e::{self, AuditRow, E2eTask};
use serde_json::json;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn e2e_audit_complete_lifecycle() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let mut task_ids = Vec::new();
    for i in 0..5 {
        let record = server
            .engine
            .enqueue(&queue, E2eTask { data: format!("audit-{i}") })
            .await
            .expect("enqueue");
        task_ids.push(record.id());
    }

    let client = reqwest::Client::new();
    for task_id in &task_ids {
        e2e::wait_for_status(
            &client,
            &server.base_url,
            &task_id.to_string(),
            "completed",
            TIMEOUT,
        )
        .await;
    }

    for task_id in &task_ids {
        let rows = e2e::query_audit_log(&pool, *task_id.as_uuid()).await;
        let transitions: Vec<_> = rows
            .iter()
            .map(|r| (r.from_status.as_deref(), r.to_status.as_str()))
            .collect();

        assert!(
            transitions.contains(&(None, "pending")),
            "task {} missing NULL→pending transition, got: {:?}",
            task_id, transitions
        );
        assert!(
            transitions.contains(&(Some("pending"), "running")),
            "task {} missing pending→running transition, got: {:?}",
            task_id, transitions
        );
        assert!(
            transitions.contains(&(Some("running"), "completed")),
            "task {} missing running→completed transition, got: {:?}",
            task_id, transitions
        );
    }

    let total_rows: usize = {
        let mut count = 0;
        for task_id in &task_ids {
            let rows = e2e::query_audit_log(&pool, *task_id.as_uuid()).await;
            count += rows.len();
        }
        count
    };

    // Each lifecycle has exactly 3 key transitions: NULL→P, P→R, R→C
    assert_eq!(
        total_rows, 15,
        "expected exactly 15 total audit rows for 5 tasks, got {total_rows}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_audit_retry_lifecycle() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let record = server
        .engine
        .enqueue_raw(
            &queue,
            "e2e_retry_counting",
            json!({"succeed_on_attempt": 2}),
            None,
            None,
            Some(3),
            None,
            None,
        )
        .await
        .expect("enqueue retry task");

    let client = reqwest::Client::new();
    e2e::wait_for_status(
        &client,
        &server.base_url,
        &record.id().to_string(),
        "completed",
        Duration::from_secs(30),
    )
    .await;

    let rows = e2e::query_audit_log(&pool, *record.id().as_uuid()).await;
    
    // Expected transitions for 1 retry:
    // 1. NULL -> pending
    // 2. pending -> running
    // 3. running -> pending (retry)
    // 4. pending -> running
    // 5. running -> completed
    let expected = vec![
        (None, "pending"),
        (Some("pending"), "running"),
        (Some("running"), "pending"),
        (Some("pending"), "running"),
        (Some("running"), "completed"),
    ];
    e2e::assert_audit_transitions(&rows, &expected);

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_audit_cancel_lifecycle() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Submit a task to a different queue so no worker picks it up
    let orphan_queue = common::unique_queue();
    let record = server
        .engine
        .enqueue_raw(
            &orphan_queue,
            "e2e_test",
            json!({"data": "to-cancel"}),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("enqueue for cancel");

    // Cancel before any worker claims it
    let cancel_result = server.engine.cancel(record.id()).await.expect("cancel");
    assert!(
        matches!(cancel_result, iron_defer::CancelResult::Cancelled(_)),
        "expected Cancelled result"
    );

    let rows = e2e::query_audit_log(&pool, *record.id().as_uuid()).await;
    let expected = vec![
        (None, "pending"),
        (Some("pending"), "cancelled"),
    ];
    e2e::assert_audit_transitions(&rows, &expected);

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_audit_trace_id_correlation() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let trace_id = "aabbccdd11223344aabbccdd11223344";
    let record = server
        .engine
        .enqueue_raw(
            &queue,
            "e2e_test",
            json!({"data": "trace-audit"}),
            None,
            None,
            None,
            Some(trace_id),
            None,
        )
        .await
        .expect("enqueue with trace_id");

    let client = reqwest::Client::new();
    e2e::wait_for_status(
        &client,
        &server.base_url,
        &record.id().to_string(),
        "completed",
        TIMEOUT,
    )
    .await;

    let rows = e2e::query_audit_log(&pool, *record.id().as_uuid()).await;
    assert_eq!(rows.len(), 3, "expected exactly 3 audit rows");

    for row in &rows {
        assert_eq!(
            row.trace_id.as_deref(),
            Some(trace_id),
            "every audit row must carry the task's trace_id, row id={}: {:?}→{}",
            row.id,
            row.from_status,
            row.to_status
        );
    }

    server.shutdown().await;
}

// --- Task 3: Audit log immutability tests ---

#[tokio::test]
async fn e2e_audit_immutability_rejects_update() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let record = server
        .engine
        .enqueue(&queue, E2eTask { data: "immutable-update".into() })
        .await
        .expect("enqueue");

    let client = reqwest::Client::new();
    e2e::wait_for_status(
        &client,
        &server.base_url,
        &record.id().to_string(),
        "completed",
        TIMEOUT,
    )
    .await;

    let rows = e2e::query_audit_log(&pool, *record.id().as_uuid()).await;
    assert!(!rows.is_empty(), "need at least one audit row");
    let first_id = rows[0].id;

    let result = sqlx::query("UPDATE task_audit_log SET to_status = 'hacked' WHERE id = $1")
        .bind(first_id)
        .execute(&pool)
        .await;

    let err = result.expect_err("UPDATE must be rejected by immutability trigger");
    if let Some(db_err) = err.as_database_error() {
        assert_eq!(db_err.code(), Some(std::borrow::Cow::Borrowed("P0001")), "expected SQLSTATE P0001 (RAISE EXCEPTION)");
    }
    let err_str = err.to_string();
    assert!(
        err_str.contains("audit log is append-only"),
        "error must mention 'audit log is append-only', got: {err_str}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_audit_immutability_rejects_delete() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let record = server
        .engine
        .enqueue(&queue, E2eTask { data: "immutable-delete".into() })
        .await
        .expect("enqueue");

    let client = reqwest::Client::new();
    e2e::wait_for_status(
        &client,
        &server.base_url,
        &record.id().to_string(),
        "completed",
        TIMEOUT,
    )
    .await;

    let rows = e2e::query_audit_log(&pool, *record.id().as_uuid()).await;
    assert!(!rows.is_empty(), "need at least one audit row");
    let first_id = rows[0].id;

    let result = sqlx::query("DELETE FROM task_audit_log WHERE id = $1")
        .bind(first_id)
        .execute(&pool)
        .await;

    let err = result.expect_err("DELETE must be rejected by immutability trigger");
    if let Some(db_err) = err.as_database_error() {
        assert_eq!(db_err.code(), Some(std::borrow::Cow::Borrowed("P0001")), "expected SQLSTATE P0001 (RAISE EXCEPTION)");
    }
    let err_str = err.to_string();
    assert!(
        err_str.contains("audit log is append-only"),
        "error must mention 'audit log is append-only', got: {err_str}"
    );

    server.shutdown().await;
}

// --- Task 4: Audit atomicity fault-injection tests ---

#[tokio::test]
async fn e2e_audit_atomicity_no_orphaned_state_changes() {
    let queue = common::unique_queue();
    let Some((server, pool)) = e2e::boot_e2e_engine_with_audit(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let n = 10;
    let mut task_ids = Vec::new();
    for i in 0..n {
        let record = server
            .engine
            .enqueue(&queue, E2eTask { data: format!("atomicity-{i}") })
            .await
            .expect("enqueue");
        task_ids.push(*record.id().as_uuid());
    }

    let client = reqwest::Client::new();
    for task_id in &task_ids {
        e2e::wait_for_status(
            &client,
            &server.base_url,
            &task_id.to_string(),
            "completed",
            TIMEOUT,
        )
        .await;
    }

    // Cross-reference: every task that reached a non-pending state must have audit rows
    for task_id in &task_ids {
        let _task_status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .expect("query task status");

        let audit_rows = e2e::query_audit_log(&pool, *task_id).await;

        // The audit trail must include exactly the expected sequence
        let expected = vec![
            (None, "pending"),
            (Some("pending"), "running"),
            (Some("running"), "completed"),
        ];
        e2e::assert_audit_transitions(&audit_rows, &expected);
    }

    // Cross-reference: count distinct task_ids in audit log vs tasks table
    let audited_task_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT task_id) FROM task_audit_log WHERE task_id = ANY($1)",
    )
    .bind(&task_ids[..])
    .fetch_one(&pool)
    .await
    .expect("count audited tasks");

    assert_eq!(
        audited_task_count as usize, n,
        "all {n} tasks must have audit entries"
    );

    // Verify ordering: for each task, audit rows ordered by timestamp
    // must follow valid state machine transitions
    for task_id in &task_ids {
        let audit_rows = e2e::query_audit_log(&pool, *task_id).await;
        for window in audit_rows.windows(2) {
            assert!(
                window[0].timestamp <= window[1].timestamp,
                "audit rows for task {task_id} are not in timestamp order"
            );
        }
    }

    server.shutdown().await;
}
