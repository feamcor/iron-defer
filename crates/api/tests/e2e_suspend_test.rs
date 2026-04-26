mod common;

use common::e2e::{
    boot_e2e_engine_with_suspend, query_audit_log, query_checkpoint, wait_for_status,
    assert_audit_transitions, E2eTask, SuspendableTask,
};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn e2e_suspend_signal_resume_round_trip() {
    let queue = common::unique_queue();
    let Some((ts, pool)) = boot_e2e_engine_with_suspend(&queue, Duration::from_secs(60), Duration::from_secs(60), false).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_suspendable",
            "payload": {"should_suspend": true}
        }))
        .send()
        .await
        .expect("create");
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id = body["id"].as_str().unwrap().to_owned();

    // Wait for suspended
    wait_for_status(&client, &ts.base_url, &task_id, "suspended", Duration::from_secs(10)).await;

    // Send signal
    let signal_resp = client
        .post(ts.url(&format!("/tasks/{task_id}/signal")))
        .json(&json!({"payload": {"approval": "approved"}}))
        .send()
        .await
        .expect("signal");
    assert_eq!(signal_resp.status(), 200);

    // Wait for completed
    let final_body = wait_for_status(&client, &ts.base_url, &task_id, "completed", Duration::from_secs(10)).await;
    assert_eq!(final_body["status"], "completed");

    // Verify signal_payload was stored
    assert!(final_body["signalPayload"].is_object());

    ts.shutdown().await;
}

#[tokio::test]
async fn e2e_concurrent_signal_race() {
    let queue = common::unique_queue();
    let Some((ts, _pool)) = boot_e2e_engine_with_suspend(&queue, Duration::from_secs(60), Duration::from_secs(60), false).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_suspendable",
            "payload": {"should_suspend": true}
        }))
        .send()
        .await
        .expect("create");
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id = body["id"].as_str().unwrap().to_owned();

    wait_for_status(&client, &ts.base_url, &task_id, "suspended", Duration::from_secs(10)).await;

    // Send 10 concurrent signals (with timeout to prevent deadlock if a spawn fails)
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(10));
    let mut handles = Vec::new();
    for _ in 0..10 {
        let b = barrier.clone();
        let c = reqwest::Client::new();
        let url = ts.url(&format!("/tasks/{task_id}/signal"));
        handles.push(tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), b.wait()).await.ok();
            c.post(&url)
                .json(&json!({"payload": {"approve": true}}))
                .send()
                .await
        }));
    }
    let mut successes = 0;
    for h in handles {
        let result = h.await.unwrap().unwrap();
        if result.status() == 200 {
            successes += 1;
        }
    }
    assert_eq!(successes, 1, "exactly one signal should succeed");

    // Verify task reaches Completed
    wait_for_status(&client, &ts.base_url, &task_id, "completed", Duration::from_secs(10)).await;

    ts.shutdown().await;
}

#[tokio::test]
async fn e2e_suspend_timeout_auto_fail() {
    let queue = common::unique_queue();
    let Some((ts, _pool)) = boot_e2e_engine_with_suspend(
        &queue,
        Duration::from_secs(2),
        Duration::from_secs(1),
        false,
    ).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_suspendable",
            "payload": {"should_suspend": true}
        }))
        .send()
        .await
        .expect("create");
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id = body["id"].as_str().unwrap().to_owned();

    wait_for_status(&client, &ts.base_url, &task_id, "suspended", Duration::from_secs(10)).await;

    // Do NOT signal. Wait for sweeper to auto-fail.
    let failed_body = wait_for_status(&client, &ts.base_url, &task_id, "failed", Duration::from_secs(15)).await;
    assert!(
        failed_body["lastError"]
            .as_str()
            .unwrap_or("")
            .contains("suspend timeout exceeded"),
        "expected 'suspend timeout exceeded' in lastError, got: {}",
        failed_body["lastError"]
    );

    ts.shutdown().await;
}

#[tokio::test]
async fn e2e_suspended_not_blocking_concurrency() {
    let queue = common::unique_queue();
    let pool = common::fresh_pool_on_shared_container().await;
    let Some(pool) = pool else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let db_url = common::test_db_url().await.unwrap().to_owned();

    let worker_config = iron_defer::WorkerConfig {
        concurrency: 1,
        poll_interval: Duration::from_millis(50),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(2),
        suspend_timeout: Duration::from_secs(60),
        ..iron_defer::WorkerConfig::default()
    };

    let engine = iron_defer::IronDefer::builder()
        .pool(pool.clone())
        .register::<SuspendableTask>()
        .register::<E2eTask>()
        .worker_config(worker_config)
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = std::sync::Arc::new(engine);
    let token = iron_defer::CancellationToken::new();

    let engine_ref = std::sync::Arc::clone(&engine);
    let worker_token = token.clone();
    let _worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Submit suspendable task — it will suspend
    let record = engine
        .enqueue_raw(&queue, "e2e_suspendable", json!({"should_suspend": true}), None, None, None, None, None)
        .await
        .expect("enqueue suspendable");
    let suspend_id = record.id();

    // Wait for it to suspend
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(r) = engine.find(suspend_id).await.unwrap() {
            if r.status() == iron_defer::TaskStatus::Suspended {
                break;
            }
        }
    }
    let r = engine.find(suspend_id).await.unwrap().unwrap();
    assert_eq!(r.status(), iron_defer::TaskStatus::Suspended);

    // Now submit a plain E2eTask — with concurrency=1, if suspended task blocks,
    // this will never be claimed
    let e2e_record = engine
        .enqueue_raw(&queue, "e2e_test", json!({"data": "second"}), None, None, None, None, None)
        .await
        .expect("enqueue e2e");
    let e2e_id = e2e_record.id();

    // Wait for it to complete
    let mut completed = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(r) = engine.find(e2e_id).await.unwrap() {
            if r.status() == iron_defer::TaskStatus::Completed {
                completed = true;
                break;
            }
        }
    }
    token.cancel();
    assert!(completed, "E2eTask should be claimed despite suspended task holding concurrency=1 slot");
}

#[tokio::test]
async fn e2e_suspend_checkpoint_survives() {
    let queue = common::unique_queue();
    let Some((ts, pool)) = boot_e2e_engine_with_suspend(&queue, Duration::from_secs(60), Duration::from_secs(60), false).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_suspendable",
            "payload": {"should_suspend": true}
        }))
        .send()
        .await
        .expect("create");
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id_str = body["id"].as_str().unwrap().to_owned();
    let task_id_uuid: uuid::Uuid = task_id_str.parse().unwrap();

    wait_for_status(&client, &ts.base_url, &task_id_str, "suspended", Duration::from_secs(10)).await;

    // Verify checkpoint is persisted
    let checkpoint = query_checkpoint(&pool, task_id_uuid).await;
    assert!(checkpoint.is_some(), "checkpoint should be persisted during suspend");
    let cp = checkpoint.unwrap();
    assert_eq!(cp["step"], "pre_suspend");

    // Signal and wait for completion
    client
        .post(ts.url(&format!("/tasks/{task_id_str}/signal")))
        .json(&json!({"payload": {"resume": true}}))
        .send()
        .await
        .expect("signal");

    wait_for_status(&client, &ts.base_url, &task_id_str, "completed", Duration::from_secs(10)).await;

    ts.shutdown().await;
}

#[tokio::test]
async fn e2e_signal_non_suspended_returns_409() {
    let queue = common::unique_queue();
    let Some((ts, _pool)) = boot_e2e_engine_with_suspend(&queue, Duration::from_secs(60), Duration::from_secs(60), false).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    // Submit a non-suspending task
    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_test",
            "payload": {"data": "hello"}
        }))
        .send()
        .await
        .expect("create");
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id = body["id"].as_str().unwrap().to_owned();

    // Wait for it to complete
    wait_for_status(&client, &ts.base_url, &task_id, "completed", Duration::from_secs(10)).await;

    // Try to signal a completed task
    let signal_resp = client
        .post(ts.url(&format!("/tasks/{task_id}/signal")))
        .json(&json!({"payload": {"test": true}}))
        .send()
        .await
        .expect("signal");
    assert_eq!(signal_resp.status(), 409);

    ts.shutdown().await;
}

#[tokio::test]
async fn e2e_suspend_with_audit_log() {
    let queue = common::unique_queue();
    let Some((ts, pool)) = boot_e2e_engine_with_suspend(
        &queue,
        Duration::from_secs(60),
        Duration::from_secs(60),
        true,
    ).await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };

    let client = reqwest::Client::new();

    let resp = client
        .post(ts.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_suspendable",
            "payload": {"should_suspend": true}
        }))
        .send()
        .await
        .expect("create");
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id_str = body["id"].as_str().unwrap().to_owned();
    let task_id_uuid: uuid::Uuid = task_id_str.parse().unwrap();

    wait_for_status(&client, &ts.base_url, &task_id_str, "suspended", Duration::from_secs(10)).await;

    client
        .post(ts.url(&format!("/tasks/{task_id_str}/signal")))
        .json(&json!({"payload": {"approval": "approved"}}))
        .send()
        .await
        .expect("signal");

    wait_for_status(&client, &ts.base_url, &task_id_str, "completed", Duration::from_secs(10)).await;

    let audit_rows = query_audit_log(&pool, task_id_uuid).await;
    assert_audit_transitions(
        &audit_rows,
        &[
            (None, "pending"),
            (Some("pending"), "running"),
            (Some("running"), "suspended"),
            (Some("suspended"), "pending"),
            (Some("pending"), "running"),
            (Some("running"), "completed"),
        ],
    );

    ts.shutdown().await;
}
