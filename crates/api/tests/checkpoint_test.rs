mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    ExecutionErrorKind, IronDefer, Task, TaskContext, TaskError, TaskId, WorkerConfig,
};
use iron_defer_domain::{AttemptCount, WorkerId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use common::e2e::{self, TestServer};

const TIMEOUT: Duration = Duration::from_secs(15);

/// Task that checkpoints after each step and optionally fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointStepTask {
    total_steps: u32,
    fail_after_step: Option<u32>,
}

impl CheckpointStepTask {
    fn new(total_steps: u32) -> Self {
        Self {
            total_steps,
            fail_after_step: None,
        }
    }

    fn failing_after(total_steps: u32, fail_after: u32) -> Self {
        Self {
            total_steps,
            fail_after_step: Some(fail_after),
        }
    }
}

impl Task for CheckpointStepTask {
    const KIND: &'static str = "checkpoint_step";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        let start_step = match ctx.last_checkpoint() {
            Some(v) => v["step"].as_u64().unwrap_or(0) as u32 + 1,
            None => 1,
        };

        for step in start_step..=self.total_steps {
            ctx.checkpoint(json!({
                "step": step,
                "data": format!("result_of_step_{step}")
            }))
            .await?;

            if let Some(fail_at) = self.fail_after_step {
                if step == fail_at && ctx.attempt().get() == 1 {
                    return Err(TaskError::ExecutionFailed {
                        kind: ExecutionErrorKind::HandlerFailed {
                            source: format!("intentional failure after step {step}").into(),
                        },
                    });
                }
            }
        }
        Ok(())
    }
}

fn fast_worker_config() -> WorkerConfig {
    WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(50),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(2),
        ..WorkerConfig::default()
    }
}

async fn boot_checkpoint_engine(queue: &str) -> Option<(TestServer, PgPool)> {
    let pool = common::fresh_pool_on_shared_container().await?;
    let db_url = common::test_db_url().await?.to_owned();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<CheckpointStepTask>()
        .register::<e2e::E2eTask>()
        .worker_config(fast_worker_config())
        .queue(queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build checkpoint engine");

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

    Some((
        TestServer {
            base_url,
            engine,
            db_url,
            token,
            server_handle: Some(server_handle),
            worker_handle: Some(worker_handle),
        },
        pool,
    ))
}

async fn enqueue_checkpoint_task(
    client: &reqwest::Client,
    base_url: &str,
    queue: &str,
    task: &CheckpointStepTask,
) -> String {
    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "checkpoint_step",
            "payload": task,
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

/// AC2: Task that crashes after checkpointing resumes from last checkpoint on retry.
#[tokio::test]
async fn checkpoint_persists_and_survives_retry() {
    let queue = common::unique_queue();
    let Some((server, pool)) = boot_checkpoint_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let task = CheckpointStepTask::failing_after(5, 3);
    let task_id = enqueue_checkpoint_task(&client, &server.base_url, &queue, &task).await;

    let result = e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT checkpoint FROM tasks WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(&task_id).unwrap())
    .fetch_optional(&pool)
    .await
    .expect("query");
    let checkpoint = row.expect("task exists").0;
    assert!(checkpoint.is_none(), "checkpoint should be cleared after completion");

    server.shutdown().await;
}

/// AC3: First attempt with no checkpoint returns None.
#[tokio::test]
async fn checkpoint_none_on_first_attempt() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = boot_checkpoint_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let task = CheckpointStepTask::new(3);
    let task_id = enqueue_checkpoint_task(&client, &server.base_url, &queue, &task).await;

    let result = e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    server.shutdown().await;
}

/// AC4: Checkpoint data is cleared on completion.
#[tokio::test]
async fn checkpoint_cleared_on_completion() {
    let queue = common::unique_queue();
    let Some((server, pool)) = boot_checkpoint_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let task = CheckpointStepTask::new(3);
    let task_id = enqueue_checkpoint_task(&client, &server.base_url, &queue, &task).await;

    let result = e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let row: (Option<serde_json::Value>,) = sqlx::query_as(
        "SELECT checkpoint FROM tasks WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(&task_id).unwrap())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert!(row.0.is_none(), "checkpoint should be NULL after completion");

    server.shutdown().await;
}

/// AC5: REST API includes lastCheckpoint field.
#[tokio::test]
async fn checkpoint_visible_in_rest_api() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = boot_checkpoint_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let task = CheckpointStepTask::new(3);
    let task_id = enqueue_checkpoint_task(&client, &server.base_url, &queue, &task).await;

    let result = e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert!(result.get("lastCheckpoint").is_some(), "response must include lastCheckpoint field");
    assert!(result["lastCheckpoint"].is_null(), "lastCheckpoint should be null after completion");

    server.shutdown().await;
}

/// Checkpoint size limit: reject payloads exceeding 1 MiB.
#[tokio::test]
async fn checkpoint_size_limit() {
    let ctx = TaskContext::new(
        TaskId::new(),
        WorkerId::new(),
        AttemptCount::new(1).unwrap(),
    );
    let large_data = json!({"data": "x".repeat(1_100_000)});
    let result = ctx.checkpoint(large_data).await;
    assert!(result.is_err(), "should reject oversized checkpoint");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("exceeds maximum") || msg.contains("1 MiB"),
        "error should mention size limit: {msg}"
    );
}

/// Checkpoint multiple overwrites: only last value persists.
#[tokio::test]
async fn checkpoint_multiple_overwrites() {
    let queue = common::unique_queue();
    let Some((server, pool)) = boot_checkpoint_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let task = CheckpointStepTask::new(5);
    let task_id = enqueue_checkpoint_task(&client, &server.base_url, &queue, &task).await;

    let result = e2e::wait_for_status(&client, &server.base_url, &task_id, "completed", TIMEOUT).await;
    assert_eq!(result["status"], "completed");

    let row: (Option<serde_json::Value>,) = sqlx::query_as(
        "SELECT checkpoint FROM tasks WHERE id = $1",
    )
    .bind(uuid::Uuid::parse_str(&task_id).unwrap())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert!(row.0.is_none(), "checkpoint should be cleared after completion (was overwritten 5 times during execution)");

    server.shutdown().await;
}
