//! E2E readiness probe under DB outage (Story 8.3, AC 4).
//!
//! Uses an isolated chaos container so stop/start is safe.
//! Tests that `GET /health/ready` transitions 200→503→200 across
//! a DB outage cycle, and that pending tasks complete after recovery.

mod chaos_common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReadinessTask {
    n: usize,
}

impl Task for ReadinessTask {
    const KIND: &'static str = "readiness_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[tokio::test]
async fn e2e_readiness_probe_db_outage_cycle() {
    if chaos_common::should_skip() {
        eprintln!("[skip] IRON_DEFER_SKIP_DOCKER_CHAOS set");
        return;
    }

    let (pool, container, _url, _port) = chaos_common::boot_isolated_chaos_db().await;
    let queue = chaos_common::unique_queue();

    let config = WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(100),
        sweeper_interval: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(5),
        lease_duration: Duration::from_secs(5),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<ReadinessTask>()
        .worker_config(config)
        .queue(&queue)
        .readiness_timeout(Duration::from_secs(2))
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    // Submit tasks before outage
    for i in 0..5 {
        engine
            .enqueue(&queue, ReadinessTask { n: i })
            .await
            .expect("enqueue");
    }

    // Start workers
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
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

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");

    // Verify readiness is 200 before outage
    let resp = client
        .get(format!("{base_url}/health/ready"))
        .send()
        .await
        .expect("health/ready");
    assert_eq!(resp.status(), 200, "readiness should be 200 before outage");

    // --- OUTAGE ---
    container.stop().await.expect("stop container");

    // Poll for 503
    let mut saw_503 = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(resp) = client.get(format!("{base_url}/health/ready")).send().await {
            if resp.status() == 503 {
                saw_503 = true;
                break;
            }
        }
    }
    assert!(
        saw_503,
        "GET /health/ready should return 503 during DB outage"
    );

    // --- RECOVERY ---
    container.start().await.expect("restart container");

    // Poll for 200 (within 30s)
    let mut saw_200 = false;
    for _ in 0..45 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(resp) = client.get(format!("{base_url}/health/ready")).send().await {
            if resp.status() == 200 {
                saw_200 = true;
                break;
            }
        }
    }
    assert!(
        saw_200,
        "GET /health/ready should return 200 within 45s after recovery"
    );

    // Verify pending tasks complete after recovery
    let mut all_done = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let tasks = engine.list(&queue).await.unwrap_or_default();
        if !tasks.is_empty()
            && tasks
                .iter()
                .all(|t| t.status() == iron_defer::TaskStatus::Completed)
        {
            all_done = true;
            break;
        }
    }
    assert!(
        all_done,
        "pending tasks should complete after DB recovery"
    );

    token.cancel();
    // In-process server shutdown
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}
