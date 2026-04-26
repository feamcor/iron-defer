mod common;

use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use std::sync::{Mutex, LazyLock};
use tokio::sync::Notify;

static VISIBILITY_NOTIFY: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

use iron_defer::{
    CancellationToken, DatabaseConfig, ExecutionErrorKind, IronDefer, Task, TaskContext, TaskError,
    WorkerConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use common::e2e::{self, CheckpointStepTask, TestServer};

const TIMEOUT: Duration = Duration::from_secs(20);

async fn enqueue_checkpoint_task_with_max_attempts(
    client: &reqwest::Client,
    base_url: &str,
    queue: &str,
    task: &CheckpointStepTask,
    max_attempts: u32,
) -> String {
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_checkpoint_step",
            "payload": task,
            "maxAttempts": max_attempts,
        }))
        .send()
        .await
        .expect("enqueue");
    assert!(
        resp.status().is_success(),
        "enqueue failed: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("json");
    body["id"].as_str().expect("task id").to_string()
}

// ---------------------------------------------------------------------------
// Task 1: Checkpoint crash-recovery E2E tests (AC: 1)
// ---------------------------------------------------------------------------

/// AC1: Multi-step task crashes after step 3, resumes from step 4 on retry.
///
/// Fail pattern: (1, 3) and (2, 3) — both attempts fail at step 3.
/// With maxAttempts=2: if resume works, attempt 2 starts at step 4 (skips 3) → completed.
/// Without resume, attempt 2 re-runs step 3 and crashes → failed.
#[tokio::test]
async fn e2e_checkpoint_crash_recovery() {
    let queue = common::unique_queue();
    let task = CheckpointStepTask::with_failures(5, vec![(1, 3), (2, 3)]);
    let Some((server, pool)) = e2e::boot_e2e_engine_with_checkpoint(&queue, &task).await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let client = reqwest::Client::new();
    let task_id = enqueue_checkpoint_task_with_max_attempts(
        &client,
        &server.base_url,
        &queue,
        &task,
        2,
    )
    .await;

    let result =
        e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let cp =
        e2e::query_checkpoint(&pool, uuid::Uuid::parse_str(&task_id).unwrap()).await;
    assert!(cp.is_none(), "checkpoint should be cleared after completion");

    server.shutdown().await;
}

/// AC1: Multiple retries with checkpoints at different crash points.
///
/// Fail pattern: (1,2), (2,2), (2,4), (3,2), (3,4) with maxAttempts=3.
/// With resume: attempt 1→step 2 crash, attempt 2→resume from 3→step 4 crash,
/// attempt 3→resume from 5→complete.
/// Without resume: attempt 2 re-starts at 1, hits (2,2)→crash, attempt 3
/// resumes from CP=2→step 3→step 4→hit (3,4)→crash→failed.
#[tokio::test]
async fn e2e_checkpoint_multiple_retries() {
    let queue = common::unique_queue();
    let task = CheckpointStepTask::with_failures(
        6,
        vec![(1, 2), (2, 2), (2, 4), (3, 2), (3, 4)],
    );
    let Some((server, pool)) = e2e::boot_e2e_engine_with_checkpoint(&queue, &task).await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let client = reqwest::Client::new();
    let task_id = enqueue_checkpoint_task_with_max_attempts(
        &client,
        &server.base_url,
        &queue,
        &task,
        3,
    )
    .await;

    let result =
        e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let cp =
        e2e::query_checkpoint(&pool, uuid::Uuid::parse_str(&task_id).unwrap()).await;
    assert!(cp.is_none(), "checkpoint cleared after completion");

    server.shutdown().await;
}

/// AC1: First attempt sees None checkpoint, checkpoints, and completes.
#[tokio::test]
async fn e2e_checkpoint_none_first_attempt() {
    let queue = common::unique_queue();
    let task = CheckpointStepTask::new(3);
    let Some((server, pool)) = e2e::boot_e2e_engine_with_checkpoint(&queue, &task).await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let client = reqwest::Client::new();
    let task_id = enqueue_checkpoint_task_with_max_attempts(
        &client,
        &server.base_url,
        &queue,
        &task,
        1,
    )
    .await;

    let result =
        e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    // lastCheckpoint null after completion (via REST)
    assert!(
        result["lastCheckpoint"].is_null(),
        "lastCheckpoint should be null after completion"
    );

    // Confirm via direct DB query
    let cp =
        e2e::query_checkpoint(&pool, uuid::Uuid::parse_str(&task_id).unwrap()).await;
    assert!(cp.is_none(), "checkpoint NULL in DB after completion");

    server.shutdown().await;
}

/// AC1: Large checkpoint payload (512 KiB) survives crash and retry.
///
/// With maxAttempts=2 and fail on both attempts at step 1 (without resume):
/// attempt 1 checkpoints 512 KiB, crashes. Attempt 2 with resume skips step 1,
/// verifies payload in checkpoint, completes.
#[tokio::test]
async fn e2e_checkpoint_large_payload() {
    let queue = common::unique_queue();

    let pool = match common::fresh_pool_on_shared_container().await {
        Some(p) => p,
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };
    let db_url = match common::test_db_url().await {
        Some(u) => u.to_owned(),
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<LargeCheckpointTask>()
        .worker_config(WorkerConfig {
            concurrency: 2,
            poll_interval: Duration::from_millis(50),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let server = TestServer {
        base_url: base_url.clone(),
        engine,
        db_url,
        token,
        server_handle: Some(server_handle),
        worker_handle: Some(worker_handle),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_large_checkpoint",
            "payload": { "payload_size": 512 * 1024 },
            "maxAttempts": 3,
        }))
        .send()
        .await
        .expect("enqueue");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json");
    let task_id = body["id"].as_str().expect("task id").to_string();

    let result =
        e2e::wait_for_status(&client, &base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let cp =
        e2e::query_checkpoint(&pool, uuid::Uuid::parse_str(&task_id).unwrap()).await;
    assert!(cp.is_none(), "checkpoint cleared after completion");

    server.shutdown().await;
}

/// AC1: Checkpoint data visible via REST API while task is running.
#[tokio::test]
async fn e2e_checkpoint_visible_in_rest() {
    let queue = common::unique_queue();
    let sync_id = uuid::Uuid::new_v4().to_string();
    let notify = {
        let mut map = VISIBILITY_NOTIFY.lock().unwrap();
        let n = Arc::new(Notify::new());
        map.insert(sync_id.clone(), n.clone());
        n
    };

    let pool = match common::fresh_pool_on_shared_container().await {
        Some(p) => p,
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };
    let db_url = match common::test_db_url().await {
        Some(u) => u.to_owned(),
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SlowCheckpointTask>()
        .worker_config(WorkerConfig {
            concurrency: 2,
            poll_interval: Duration::from_millis(50),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let server = TestServer {
        base_url: base_url.clone(),
        engine,
        db_url,
        token,
        server_handle: Some(server_handle),
        worker_handle: Some(worker_handle),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_slow_checkpoint",
            "payload": { "sync_id": sync_id },
        }))
        .send()
        .await
        .expect("enqueue");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json");
    let task_id = body["id"].as_str().expect("task id").to_string();

    // Poll until checkpoint is visible via REST (task is running + checkpoint written)
    let start = std::time::Instant::now();
    loop {
        let resp = client
            .get(format!("{base_url}/tasks/{task_id}"))
            .send()
            .await
            .expect("poll");
        let body: serde_json::Value = resp.json().await.expect("json");
        if body["status"] == "running" && !body["lastCheckpoint"].is_null() {
            assert!(
                body.get("lastCheckpoint").is_some(),
                "response must include lastCheckpoint field"
            );
            assert_eq!(
                body["lastCheckpoint"]["step"], 1,
                "checkpoint should be from step 1"
            );
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!(
                "timed out waiting for checkpoint to become visible; last status: {}, lastCheckpoint: {}",
                body["status"], body["lastCheckpoint"]
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Signal task to complete
    notify.notify_one();

    // Wait for completion
    let result = e2e::wait_for_status(&client, &base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Task 2: Sweeper interaction E2E test (AC: 1)
// ---------------------------------------------------------------------------

/// AC1: Sweeper recovers zombie task, retry resumes from last checkpoint.
#[tokio::test]
async fn e2e_checkpoint_sweeper_recovery() {
    let queue = common::unique_queue();

    let pool = match common::fresh_pool_on_shared_container().await {
        Some(p) => p,
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };
    let db_url = match common::test_db_url().await {
        Some(u) => u.to_owned(),
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SweeperCheckpointTask>()
        .worker_config(WorkerConfig {
            concurrency: 2,
            poll_interval: Duration::from_millis(50),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            lease_duration: Duration::from_secs(2),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .sweeper_interval(Duration::from_secs(1))
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build sweeper engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let server = TestServer {
        base_url: base_url.clone(),
        engine,
        db_url,
        token,
        server_handle: Some(server_handle),
        worker_handle: Some(worker_handle),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_sweeper_checkpoint",
            "payload": {},
            "maxAttempts": 5,
        }))
        .send()
        .await
        .expect("enqueue");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json");
    let task_id = body["id"].as_str().expect("task id").to_string();

    let result =
        e2e::wait_for_status(&client, &base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let cp =
        e2e::query_checkpoint(&pool, uuid::Uuid::parse_str(&task_id).unwrap()).await;
    assert!(cp.is_none(), "checkpoint cleared after completion");

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Task 3: Audit log integration test (AC: 1)
// ---------------------------------------------------------------------------

/// Checkpoint writes do NOT produce audit rows. Clean lifecycle: 3 audit rows.
#[tokio::test]
async fn e2e_checkpoint_with_audit_log() {
    let queue = common::unique_queue();

    let pool = match common::fresh_pool_on_shared_container().await {
        Some(p) => p,
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };
    let db_url = match common::test_db_url().await {
        Some(u) => u.to_owned(),
        None => {
            eprintln!("[skip] Docker not available");
            return;
        }
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<CheckpointStepTask>()
        .worker_config(WorkerConfig {
            concurrency: 2,
            poll_interval: Duration::from_millis(50),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .database_config(DatabaseConfig {
            audit_log: true,
            ..DatabaseConfig::default()
        })
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build audit engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let server = TestServer {
        base_url: base_url.clone(),
        engine,
        db_url,
        token,
        server_handle: Some(server_handle),
        worker_handle: Some(worker_handle),
    };

    let task = CheckpointStepTask::new(3);
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_checkpoint_step",
            "payload": task,
        }))
        .send()
        .await
        .expect("enqueue");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json");
    let task_id = body["id"].as_str().expect("task id").to_string();

    let result =
        e2e::wait_for_status(&client, &base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let task_uuid = uuid::Uuid::parse_str(&task_id).unwrap();
    let audit_rows = e2e::query_audit_log(&pool, task_uuid).await;
    e2e::assert_audit_transitions(
        &audit_rows,
        &[
            (None, "pending"),
            (Some("pending"), "running"),
            (Some("running"), "completed"),
        ],
    );
    assert_eq!(
        audit_rows.len(),
        3,
        "3 checkpoint writes must produce zero additional audit rows; got {} rows",
        audit_rows.len()
    );

    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Helper task types
// ---------------------------------------------------------------------------

/// Checkpoints a large payload (configurable size), crashes on attempt 1,
/// verifies the payload survives intact on retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LargeCheckpointTask {
    payload_size: usize,
}

impl Task for LargeCheckpointTask {
    const KIND: &'static str = "e2e_large_checkpoint";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        if ctx.attempt().get() == 1 {
            let large_string = "x".repeat(self.payload_size);
            ctx.checkpoint(json!({"data": large_string})).await?;
            return Err(TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: "intentional crash after large checkpoint".into(),
                },
            });
        }

        let cp = ctx
            .last_checkpoint()
            .ok_or_else(|| TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: "missing checkpoint on retry".into(),
                },
            })?;
        let data = cp.get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: "missing or invalid data field in checkpoint".into(),
                },
            })?;
        assert_eq!(data.len(), self.payload_size, "payload must survive intact");
        assert!(data.chars().all(|c| c == 'x'), "payload content corrupted");

        Ok(())
    }
}

/// Checkpoints at step 1, then waits for notification before completing.
/// Used to test checkpoint visibility via REST while the task is still running.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowCheckpointTask {
    pub sync_id: String,
}

impl Task for SlowCheckpointTask {
    const KIND: &'static str = "e2e_slow_checkpoint";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        ctx.checkpoint(json!({"step": 1, "data": "visible_checkpoint"}))
            .await?;

        let notify = {
            let mut map = VISIBILITY_NOTIFY.lock().unwrap();
            map.entry(self.sync_id.clone())
                .or_insert_with(|| Arc::new(Notify::new()))
                .clone()
        };

        // Wait for test to signal it has seen the checkpoint
        notify.notified().await;
        Ok(())
    }
}

/// Checkpoints at step 2, then sleeps indefinitely past lease_duration to
/// trigger sweeper recovery. On retry, resumes from checkpoint and completes.
/// The sleeping handler is cancelled during server shutdown (no race with retry).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SweeperCheckpointTask {}

impl Task for SweeperCheckpointTask {
    const KIND: &'static str = "e2e_sweeper_checkpoint";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        if ctx.attempt().get() == 1 {
            ctx.checkpoint(json!({"step": 1, "data": "step_1_result"}))
                .await?;
            ctx.checkpoint(json!({"step": 2, "data": "step_2_result"}))
                .await?;
            // Sleep well past lease_duration (2s) to become a zombie.
            // Sweeper recovers the task; this handler is cancelled on shutdown.
            tokio::time::sleep(Duration::from_secs(120)).await;
            return Ok(());
        }

        let cp = ctx
            .last_checkpoint()
            .ok_or_else(|| TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: "missing checkpoint after sweeper recovery".into(),
                },
            })?;
        let step = cp.get("step")
            .and_then(|s| s.as_u64())
            .ok_or_else(|| TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: "missing or invalid step field in checkpoint".into(),
                },
            })?;
        assert_eq!(step, 2, "should resume from step 2 checkpoint");

        ctx.checkpoint(json!({"step": 3, "data": "step_3_result"}))
            .await?;

        Ok(())
    }
}
