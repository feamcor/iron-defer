//! E2E metrics scrape verification (Story 8.3, AC 2).
//!
//! Boots a full engine with OTel/Prometheus configured, processes tasks,
//! then scrapes `GET /metrics` to verify metric presence and format.

mod common;

use std::sync::Arc;

use common::otel::build_harness;
use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricsE2eTask {
    value: i32,
}

impl Task for MetricsE2eTask {
    const KIND: &'static str = "metrics_e2e";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_metrics_scrape_after_task_processing() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();
    let harness = build_harness();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<MetricsE2eTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    // Submit tasks
    for i in 0..3 {
        engine
            .enqueue(&queue, MetricsE2eTask { value: i })
            .await
            .expect("enqueue");
    }

    // Start workers and wait for completion
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    let all_done = common::otel::await_all_terminal(
        &engine,
        &queue,
        50,
        std::time::Duration::from_millis(200),
    )
    .await;
    assert!(all_done, "tasks did not complete within timeout");

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker_handle).await;

    // Start HTTP server for scrape
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = CancellationToken::new();
    let server_cancel = server_token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_cancel.cancelled_owned())
            .await
            .expect("server");
    });

    // Scrape /metrics
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base_url}/metrics"))
        .send()
        .await
        .expect("GET /metrics");

    assert_eq!(resp.status(), 200);

    // Validate Prometheus text exposition format
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("content-type")
        .to_str()
        .expect("content-type str");
    assert!(
        content_type.contains("text/plain") && content_type.contains("version=0.0.4"),
        "expected Prometheus text format, got {content_type}"
    );

    let body = resp.text().await.expect("body");

    // AC2: task_attempts_total counter incremented
    assert!(
        body.contains("iron_defer_task_attempts"),
        "missing iron_defer_task_attempts"
    );

    // AC2: task_duration_seconds histogram with queue, kind, status labels
    assert!(
        body.contains("iron_defer_task_duration_seconds"),
        "missing iron_defer_task_duration_seconds"
    );

    // Verify labels on duration histogram (OTel may double the _seconds suffix)
    assert!(
        body.lines().any(|l| l.contains("iron_defer_task_duration_seconds") && l.contains("_count") && l.contains("status=\"completed\"")),
        "duration histogram count line with status=completed not found"
    );

    // Verify attempts counter has labels and value >= 3 (OTel may add _total suffix)
    let attempts_re = regex::Regex::new(r#"iron_defer_task_attempts.*\{.*\} ([0-9.]+)$"#).unwrap();
    let attempts_line = body
        .lines()
        .find(|l| attempts_re.is_match(l) && !l.starts_with('#'))
        .expect("attempts counter line");
    
    let caps = attempts_re.captures(attempts_line).unwrap();
    let attempts_value: f64 = caps.get(1).map(|m| m.as_str().parse().unwrap_or(0.0)).unwrap_or(0.0);
    assert!(
        attempts_value >= 3.0,
        "expected at least 3 attempts, got {attempts_value}"
    );

    // Verify format: HELP and TYPE lines present for iron_defer metrics
    assert!(
        body.lines()
            .any(|l| l.starts_with("# HELP iron_defer_")),
        "missing HELP lines for iron_defer metrics"
    );
    assert!(
        body.lines()
            .any(|l| l.starts_with("# TYPE iron_defer_")),
        "missing TYPE lines for iron_defer metrics"
    );

    server_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle).await;
    harness.provider.shutdown().expect("meter provider shutdown");
}
