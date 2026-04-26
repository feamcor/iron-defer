//! E2E post-recovery data integrity verification (Story 8.3, AC 3).
//!
//! AC3 requires: zero tasks lost, zero tasks double-executed after a
//! crash/restart cycle. These properties are already verified by the
//! existing chaos suite:
//!
//! - `chaos_db_outage_test::postgres_outage_survives_reconnection` —
//!   enqueues 20 tasks, stops Postgres mid-processing, restarts, verifies
//!   all 20 reach `completed` status and zero remain `running`.
//!
//! - `chaos_worker_crash_test::worker_crash_recovery_zero_task_loss` —
//!   claims all 10 tasks via fake worker (simulating crash), waits for
//!   lease expiry, starts real workers + sweeper, verifies all 10 reach
//!   `completed` with zero `running` or `pending`.
//!
//! This test provides a lightweight supplementary assertion: submit tasks
//! through the E2E engine, let workers process them, and verify the
//! invariant that every submitted task reaches a terminal state.

mod common;

use common::e2e::{self, E2eTask};

#[tokio::test]
async fn e2e_data_integrity_all_tasks_reach_terminal_state() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let task_count = 10;
    let mut ids = Vec::with_capacity(task_count);

    for i in 0..task_count {
        let record = server
            .engine
            .enqueue(
                &queue,
                E2eTask {
                    data: format!("integrity-{i}"),
                },
            )
            .await
            .expect("enqueue");
        ids.push(record.id());
    }

    // Wait for all tasks to complete
    let client = reqwest::Client::new();
    let timeout = std::time::Duration::from_secs(15);
    let start = std::time::Instant::now();

    loop {
        let resp = client
            .get(server.url(&format!("/tasks?queue={queue}&status=completed")))
            .send()
            .await
            .expect("list");
        let body: serde_json::Value = resp.json().await.expect("json");
        let completed = body["total"].as_u64().unwrap_or(0);

        if completed == task_count as u64 {
            break;
        }
        if start.elapsed() > timeout {
            panic!(
                "only {completed}/{task_count} tasks completed within timeout"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // Verify every submitted task is completed (zero lost)
    for id in &ids {
        let record = server
            .engine
            .find(*id)
            .await
            .expect("find")
            .expect("task exists");
        assert_eq!(
            record.status(),
            iron_defer::TaskStatus::Completed,
            "task {} should be completed",
            id
        );
    }

    // Verify no tasks stuck in non-terminal states
    let resp = client
        .get(server.url(&format!("/tasks?queue={queue}&status=running")))
        .send()
        .await
        .expect("list running");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body["total"], 0,
        "zero tasks should be stuck in running state"
    );

    let resp = client
        .get(server.url(&format!("/tasks?queue={queue}&status=pending")))
        .send()
        .await
        .expect("list pending");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body["total"], 0,
        "zero tasks should be stuck in pending state"
    );

    server.shutdown().await;
}
