//! Story 3.2 integration test — Prometheus `/metrics` endpoint round-trip.
//!
//! Verifies that the `GET /metrics` endpoint returns Prometheus text
//! exposition format with expected metric families after a task completes.

mod common;

use iron_defer::{IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use common::otel::build_harness;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricsTestTask {
    value: i32,
}

impl Task for MetricsTestTask {
    const KIND: &'static str = "metrics_test_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

/// Happy-path: enqueue → start worker → wait for completion → scrape
/// `/metrics` → assert expected metric families are present.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn prometheus_endpoint_returns_metrics_after_task_completion() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Postgres available");
        return;
    };
    let pool = &pool;

    let queue = common::unique_queue();

    // Shared harness: fresh Prometheus registry + SdkMeterProvider +
    // `iron_defer::create_metrics` handles. Story 3.3 Task 11 DRY —
    // the hand-rolled setup here pre-dated the harness.
    let harness = build_harness();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .skip_migrations(true)
        .register::<MetricsTestTask>()
        .queue(&queue)
        .metrics(harness.metrics.clone())
        .prometheus_registry(harness.registry.clone())
        .build()
        .await
        .expect("engine build");

    let engine = Arc::new(engine);

    // Enqueue a task.
    engine
        .enqueue(&queue, MetricsTestTask { value: 42 })
        .await
        .expect("enqueue");

    // Start the worker and let it complete the task.
    let token = tokio_util::sync::CancellationToken::new();
    let cancel = token.clone();
    let engine_clone = engine.clone();
    let worker_handle = tokio::spawn(async move {
        engine_clone.start(cancel).await.expect("engine start");
    });

    // Wait for the task to complete (poll until queue is empty).
    let mut attempts = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let tasks = engine.list(&queue).await.expect("list");
        let all_done = tasks.iter().all(|t| {
            t.status() == iron_defer::TaskStatus::Completed
                || t.status() == iron_defer::TaskStatus::Failed
        });
        if all_done && !tasks.is_empty() {
            break;
        }
        attempts += 1;
        assert!(attempts <= 30, "task did not complete within timeout");
    }

    // Cancel the worker.
    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), worker_handle).await;

    // Build the axum router and hit /metrics.
    let router = iron_defer::http::router::build(engine);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let server_token = tokio_util::sync::CancellationToken::new();
    let server_cancel = server_token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_cancel.cancelled_owned())
            .await
            .expect("serve");
    });

    // Scrape /metrics.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .expect("GET /metrics");

    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("content-type header")
        .to_str()
        .expect("content-type str");
    // AC 9: spec requires Prometheus text exposition format 0.0.4. A
    // weaker `contains("text/plain")` check would miss a regression to
    // a different version parameter (code-review P8).
    assert!(
        content_type.contains("text/plain") && content_type.contains("version=0.0.4"),
        "expected `text/plain; version=0.0.4; ...`, got {content_type}"
    );

    let body = resp.text().await.expect("body");

    // Assert expected metric families are present.
    assert!(
        body.contains("iron_defer_task_attempts_total"),
        "missing iron_defer_task_attempts_total in:\n{body}"
    );
    assert!(
        body.contains("iron_defer_task_duration_seconds"),
        "missing iron_defer_task_duration_seconds in:\n{body}"
    );
    // AC 9 / code-review P4: pool_connections_total is an observable
    // gauge registered against the caller-provided Meter and should
    // appear after the first scrape exercises the callback.
    assert!(
        body.contains("iron_defer_pool_connections_total"),
        "missing iron_defer_pool_connections_total in:\n{body}"
    );
    // AC 9 / code-review P4: `task_failures_total` must be 0 for the
    // happy path. The OTel Prometheus exporter only exports counters
    // that have been incremented — absence therefore implies 0 and is
    // equivalent to the assertion. If the counter is present, the
    // exported value must be 0.
    if let Some(line) = body
        .lines()
        .find(|l| l.starts_with("iron_defer_task_failures_total{"))
    {
        let value = line
            .rsplit_once(' ')
            .map(|(_, v)| v)
            .expect("counter sample has a value");
        assert!(
            value == "0" || value == "0.0",
            "expected task_failures_total == 0 on happy path, got line: {line}"
        );
    }

    // Clean up.
    server_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle).await;

    // Honor the OTel SDK contract: flush and dispose of the meter
    // provider explicitly before it drops (code-review P9). Becomes
    // essential once an OTLP `PeriodicReader` is added to the provider.
    harness
        .provider
        .shutdown()
        .expect("meter provider shutdown");
}
