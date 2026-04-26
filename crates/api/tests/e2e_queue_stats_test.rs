//! E2E queue stats accuracy test (Story 8.3, AC 1).
//!
//! Submits tasks with a slow handler, polls `GET /queues` during
//! processing, and verifies that pending counts decrease and reach zero.

mod common;

use std::sync::Arc;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowE2eTask {
    index: usize,
}

impl Task for SlowE2eTask {
    const KIND: &'static str = "slow_e2e";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        Ok(())
    }
}

const TASK_COUNT: usize = 5;

#[tokio::test]
async fn e2e_queue_stats_pending_decreases_to_zero() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SlowE2eTask>()
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);

    // Submit tasks before starting workers
    for i in 0..TASK_COUNT {
        engine
            .enqueue(&queue, SlowE2eTask { index: i })
            .await
            .expect("enqueue");
    }

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let token = CancellationToken::new();

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    // Verify initial state: all pending
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base_url}/queues"))
        .send()
        .await
        .expect("get queues");
    assert_eq!(resp.status(), 200);
    let stats: Vec<serde_json::Value> = resp.json().await.expect("json");
    let q_stat = stats
        .iter()
        .find(|s| s["queue"].as_str() == Some(&queue))
        .expect("queue in stats");
    assert_eq!(q_stat["pending"], TASK_COUNT as u64);

    // Start workers
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Poll until pending reaches zero, tracking that it decreases
    let mut saw_decrease = false;
    let mut prev_pending = TASK_COUNT as u64;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(15);

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let resp = client
            .get(format!("{base_url}/queues"))
            .send()
            .await
            .expect("get queues");
        let stats: Vec<serde_json::Value> = resp.json().await.expect("json");

        let current_pending = stats
            .iter()
            .find(|s| s["queue"].as_str() == Some(&queue))
            .and_then(|s| s["pending"].as_u64())
            .unwrap_or(0);

        if current_pending < prev_pending {
            saw_decrease = true;
        }
        prev_pending = current_pending;

        if current_pending == 0 {
            break;
        }

        if start.elapsed() > timeout {
            panic!(
                "timed out waiting for pending to reach 0, current: {current_pending}"
            );
        }
    }

    assert!(saw_decrease, "should have observed pending count decrease");

    // Final check: queue should have zero pending
    let resp = client
        .get(format!("{base_url}/queues"))
        .send()
        .await
        .expect("get queues");
    let stats: Vec<serde_json::Value> = resp.json().await.expect("json");
    let final_pending = stats
        .iter()
        .find(|s| s["queue"].as_str() == Some(&queue))
        .and_then(|s| s["pending"].as_u64())
        .unwrap_or(0);
    assert_eq!(final_pending, 0, "all tasks should be processed");

    token.cancel();
}

#[tokio::test]
async fn e2e_queue_stats_shows_running_during_processing() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<SlowE2eTask>()
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    // Start workers first
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
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    // Submit tasks — workers will start processing immediately
    for i in 0..3 {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base_url}/tasks"))
            .json(&json!({
                "queue": queue,
                "kind": "slow_e2e",
                "payload": {"index": i}
            }))
            .send()
            .await
            .expect("post");
        assert_eq!(resp.status(), 201);
    }

    // Poll for running > 0
    let client = reqwest::Client::new();
    let mut saw_running = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let resp = client
            .get(format!("{base_url}/queues"))
            .send()
            .await
            .expect("get queues");
        let stats: Vec<serde_json::Value> = resp.json().await.expect("json");
        if let Some(q) = stats.iter().find(|s| s["queue"].as_str() == Some(&queue)) {
            if q["running"].as_u64().unwrap_or(0) > 0 {
                saw_running = true;
                break;
            }
        }
    }
    assert!(
        saw_running,
        "should observe running tasks during processing"
    );

    token.cancel();
}
