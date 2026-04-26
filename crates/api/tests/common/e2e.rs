//! Shared E2E test infrastructure for full-system integration tests.
//!
//! Provides `boot_e2e_engine()` which starts an in-process engine with
//! workers and an HTTP server on an ephemeral port, backed by the shared
//! testcontainers Postgres instance.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iron_defer::{
    CancellationToken, DatabaseConfig, ExecutionErrorKind, IronDefer, Task, TaskContext, TaskError,
    WorkerConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2eTask {
    pub data: String,
}

impl Task for E2eTask {
    const KIND: &'static str = "e2e_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

pub struct TestServer {
    pub base_url: String,
    pub engine: Arc<IronDefer>,
    pub db_url: String,
    pub token: CancellationToken,
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
    pub worker_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub async fn shutdown(mut self) {
        self.token.cancel();
        if let Some(h) = self.server_handle.take() {
            tokio::time::timeout(std::time::Duration::from_secs(5), h)
                .await
                .expect("server shutdown timed out")
                .expect("server panicked during shutdown");
        }
        if let Some(h) = self.worker_handle.take() {
            tokio::time::timeout(std::time::Duration::from_secs(5), h)
                .await
                .expect("worker shutdown timed out")
                .expect("worker panicked during shutdown");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.token.cancel();
        if let Some(h) = self.server_handle.take() {
            h.abort();
        }
        if let Some(h) = self.worker_handle.take() {
            h.abort();
        }
    }
}

/// Boot a full E2E engine: Postgres pool, workers for `queue`, and HTTP
/// server on an ephemeral port. Returns `None` when Docker is unavailable.
pub async fn boot_e2e_engine(queue: &str) -> Option<(TestServer, PgPool)> {
    let pool = super::fresh_pool_on_shared_container().await?;
    let db_url = super::test_db_url().await?.to_owned();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .queue(queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build e2e engine");

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryCountingTask {
    pub succeed_on_attempt: u32,
    #[serde(skip, default = "default_counter")]
    pub counter: Arc<AtomicU32>,
}

fn default_counter() -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(0))
}

impl Task for RetryCountingTask {
    const KIND: &'static str = "e2e_retry_counting";
    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        let attempt = ctx.attempt().get() as u32;
        self.counter.fetch_add(1, Ordering::SeqCst);
        if attempt < self.succeed_on_attempt {
            Err(TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerFailed {
                    source: format!(
                        "intentional failure on attempt {}",
                        attempt
                    )
                    .into(),
                },
            })
        } else {
            Ok(())
        }
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

/// Boot an E2E engine with audit_log enabled.
pub async fn boot_e2e_engine_with_audit(queue: &str) -> Option<(TestServer, PgPool)> {
    let pool = super::fresh_pool_on_shared_container().await?;
    let db_url = super::test_db_url().await?.to_owned();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .register::<RetryCountingTask>()
        .worker_config(fast_worker_config())
        .database_config(DatabaseConfig {
            audit_log: true,
            ..DatabaseConfig::default()
        })
        .queue(queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build e2e engine with audit");

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

/// Query audit log rows for a task directly via SQL.
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
pub struct AuditRow {
    pub id: i64,
    pub task_id: uuid::Uuid,
    pub from_status: Option<String>,
    pub to_status: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub worker_id: Option<uuid::Uuid>,
    pub trace_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn query_audit_log(pool: &PgPool, task_id: uuid::Uuid) -> Vec<AuditRow> {
    sqlx::query_as::<_, AuditRow>(
        "SELECT id, task_id, from_status, to_status, timestamp, worker_id, trace_id, metadata \
         FROM task_audit_log WHERE task_id = $1 ORDER BY timestamp ASC, id ASC",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await
    .expect("query audit log")
}

pub fn assert_audit_transitions(
    audit_rows: &[AuditRow],
    expected: &[(Option<&str>, &str)],
) {
    let actual: Vec<(Option<&str>, &str)> = audit_rows
        .iter()
        .map(|r| (r.from_status.as_deref(), r.to_status.as_str()))
        .collect();
    assert_eq!(
        actual, expected,
        "audit transition chain mismatch\n  actual:   {actual:?}\n  expected: {expected:?}"
    );
}

/// Task that executes N steps, checkpointing after each. Configurable
/// failure points per attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointStepTask {
    pub total_steps: u32,
    /// `(attempt, fail_after_step)` — on the given attempt, fail after completing the given step.
    pub fail_on: Vec<(u32, u32)>,
}

impl CheckpointStepTask {
    pub fn new(total_steps: u32) -> Self {
        Self {
            total_steps,
            fail_on: Vec::new(),
        }
    }

    pub fn with_failures(total_steps: u32, fail_on: Vec<(u32, u32)>) -> Self {
        Self {
            total_steps,
            fail_on,
        }
    }
}

impl Task for CheckpointStepTask {
    const KIND: &'static str = "e2e_checkpoint_step";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        let attempt = ctx.attempt().get() as u32;
        let start_step = match ctx.last_checkpoint() {
            Some(v) => v.get("step")
                .and_then(|s| s.as_u64())
                .unwrap_or(0) as u32 + 1,
            None => 1,
        };

        for step in start_step..=self.total_steps {
            ctx.checkpoint(json!({
                "step": step,
                "data": format!("result_of_step_{step}")
            }))
            .await?;
            if self.fail_on.iter().any(|&(a, s)| a == attempt && s == step) {
                return Err(TaskError::ExecutionFailed {
                    kind: ExecutionErrorKind::HandlerFailed {
                        source: format!(
                            "intentional failure on attempt {attempt} after step {step}"
                        )
                        .into(),
                    },
                });
            }
        }
        Ok(())
    }
}

/// Boot an E2E engine with `CheckpointStepTask` registered and fast
/// worker config for retry tests.
pub async fn boot_e2e_engine_with_checkpoint(
    queue: &str,
    _task: &CheckpointStepTask,
) -> Option<(TestServer, PgPool)> {
    let pool = super::fresh_pool_on_shared_container().await?;
    let db_url = super::test_db_url().await?.to_owned();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<CheckpointStepTask>()
        .register::<E2eTask>()
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

/// Task that suspends on first execution and completes on resume (when signal is present).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendableTask {
    pub should_suspend: bool,
    #[serde(skip)]
    pub history: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Task for SuspendableTask {
    const KIND: &'static str = "e2e_suspendable";

    async fn execute(&self, ctx: &TaskContext) -> Result<(), TaskError> {
        {
            let mut h = self.history.lock().unwrap();
            if let Some(signal) = ctx.signal_payload() {
                h.push(format!("resumed:{}", signal["data"].as_str().unwrap_or("none")));
                return Ok(());
            }
            if self.should_suspend {
                h.push("suspending".to_owned());
            } else {
                h.push("executed".to_owned());
                return Ok(());
            }
        }
        ctx.checkpoint(json!({"step": "pre_suspend", "data": "checkpoint_data"})).await?;
        ctx.suspend(None).await?;
        Ok(())
    }
}

/// Boot two engines: one regionless and one pinned to a region.
pub async fn boot_regional_engine_pair(
    queue: &str,
    region: &str,
) -> Option<(TestServer, TestServer, PgPool)> {
    let pool = super::fresh_pool_on_shared_container().await?;
    
    let engine1 = boot_e2e_engine_with_config(
        pool.clone(),
        queue,
        WorkerConfig {
            region: None,
            poll_interval: Duration::from_millis(50),
            ..WorkerConfig::default()
        },
        false,
    ).await?;

    let engine2 = boot_e2e_engine_with_config(
        pool.clone(),
        queue,
        WorkerConfig {
            region: Some(region.to_owned()),
            poll_interval: Duration::from_millis(50),
            ..WorkerConfig::default()
        },
        false,
    ).await?;

    Some((engine1, engine2, pool))
}

async fn boot_e2e_engine_with_config(
    pool: PgPool,
    queue: &str,
    worker_config: WorkerConfig,
    audit_log: bool,
) -> Option<TestServer> {
    let db_url = super::test_db_url().await?.to_owned();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .worker_config(worker_config)
        .database_config(DatabaseConfig {
            audit_log,
            ..DatabaseConfig::default()
        })
        .queue(queue)
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

    Some(TestServer {
        base_url,
        engine,
        db_url,
        token,
        server_handle: Some(server_handle),
        worker_handle: Some(worker_handle),
    })
}

/// Boot an E2E engine with `SuspendableTask` registered.
/// Uses fast worker config and configurable suspend timeout + sweeper interval.
pub async fn boot_e2e_engine_with_suspend(
    queue: &str,
    suspend_timeout: Duration,
    sweeper_interval: Duration,
    audit_log: bool,
) -> Option<(TestServer, PgPool)> {
    let pool = super::fresh_pool_on_shared_container().await?;
    let db_url = super::test_db_url().await?.to_owned();

    let worker_config = WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(50),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(2),
        suspend_timeout,
        sweeper_interval,
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SuspendableTask>()
        .register::<E2eTask>()
        .worker_config(worker_config)
        .database_config(DatabaseConfig {
            audit_log,
            ..DatabaseConfig::default()
        })
        .queue(queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build suspend engine");

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

/// Query the raw checkpoint column for a task directly via SQL.
pub async fn query_checkpoint(
    pool: &PgPool,
    task_id: uuid::Uuid,
) -> Option<serde_json::Value> {
    let row: (Option<serde_json::Value>,) =
        sqlx::query_as("SELECT checkpoint FROM tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(pool)
            .await
            .expect("query checkpoint");
    row.0
}

/// Poll `GET /tasks/{id}` until the task reaches `target_status` or
/// `timeout` elapses.
pub async fn wait_for_status(
    client: &reqwest::Client,
    base_url: &str,
    task_id: &str,
    target_status: &str,
    timeout: std::time::Duration,
) -> serde_json::Value {
    let start = std::time::Instant::now();
    loop {
        let resp = client
            .get(format!("{base_url}/tasks/{task_id}"))
            .send()
            .await
            .expect("poll request");
        let body: serde_json::Value = resp.json().await.expect("poll json");
        if body["status"] == target_status {
            return body;
        }
        if start.elapsed() > timeout {
            panic!(
                "timed out waiting for task {task_id} to reach status '{target_status}', \
                 last status: {}",
                body["status"]
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
