mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    DatabaseConfig, ExecutionErrorKind, IronDefer, Task, TaskContext, TaskError, WorkerConfig,
};
use opentelemetry::global;
use opentelemetry::trace::TraceId;
use opentelemetry_sdk::testing::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;
use serde_json::json;
use serial_test::serial;

use common::e2e::{self, E2eTask, RetryCountingTask};
use common::otel::build_harness;

const TIMEOUT: Duration = Duration::from_secs(15);

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

fn traceparent(trace_id: &str) -> String {
    assert_eq!(trace_id.len(), 32, "trace_id must be 32 characters (128-bit hex)");
    format!("00-{trace_id}-b7ad6b7169203331-01")
}

async fn boot_trace_engine(
    pool: sqlx::PgPool,
    queue: &str,
) -> (Arc<IronDefer>, tokio_util::sync::CancellationToken, tokio::task::JoinHandle<()>) {
    let harness = build_harness();
    let engine = IronDefer::builder()
        .pool(pool)
        .register::<E2eTask>()
        .register::<RetryCountingTask>()
        .worker_config(fast_worker_config())
        .queue(queue)
        .metrics(harness.metrics)
        .prometheus_registry(harness.registry)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = tokio_util::sync::CancellationToken::new();
    let cancel = token.clone();
    let eng = engine.clone();
    let handle = tokio::spawn(async move {
        eng.start(cancel).await.expect("engine start");
    });
    (engine, token, handle)
}

async fn shutdown_engine(
    token: tokio_util::sync::CancellationToken,
    handle: tokio::task::JoinHandle<()>,
) {
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("worker exit")
        .expect("worker ok");
}

async fn boot_trace_e2e_server(queue: &str) -> Option<(e2e::TestServer, sqlx::PgPool)> {
    let pool = common::fresh_pool_on_shared_container().await?;
    let db_url = common::test_db_url().await?.to_owned();

    let harness = build_harness();
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .register::<RetryCountingTask>()
        .worker_config(fast_worker_config())
        .queue(queue)
        .metrics(harness.metrics)
        .prometheus_registry(harness.registry)
        .skip_migrations(true)
        .build()
        .await
        .expect("build e2e engine");

    let engine = Arc::new(engine);
    let token = iron_defer::CancellationToken::new();

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
        e2e::TestServer {
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

struct TracerGuard;
impl TracerGuard {
    fn init() -> (InMemorySpanExporter, TracerProvider) {
        let exporter = InMemorySpanExporter::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let _ = global::set_tracer_provider(provider.clone());
        (exporter, provider)
    }
}
impl Drop for TracerGuard {
    fn drop(&mut self) {
        // Do not shutdown global provider as it breaks subsequent serial tests
        // global::shutdown_tracer_provider();
    }
}

#[tokio::test]
#[serial]
async fn e2e_trace_propagation_single_task() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let (exporter, provider) = TracerGuard::init();
    let _guard = TracerGuard;

    let queue = common::unique_queue();
    let (engine, token, handle) = boot_trace_engine(pool.clone(), &queue).await;

    let trace_id_hex = "4bf92f3577b16b3edb59c6c35e764a39";
    let record = engine
        .enqueue_raw(
            &queue,
            "e2e_test",
            json!({"data": "trace-single"}),
            None,
            None,
            None,
            Some(trace_id_hex),
            None,
        )
        .await
        .expect("enqueue");

    common::otel::await_all_terminal(&engine, &queue, 40, Duration::from_millis(100)).await;
    shutdown_engine(token, handle).await;

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_span = spans
        .iter()
        .find(|s| s.name == "iron_defer.execute")
        .expect("iron_defer.execute span not found");

    let expected_trace_id = TraceId::from_hex(trace_id_hex).unwrap();
    assert_eq!(
        exec_span.span_context.trace_id(),
        expected_trace_id,
        "span must carry the submitted trace_id"
    );

    let attrs: std::collections::HashMap<String, String> = exec_span
        .attributes
        .iter()
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect();
    assert!(attrs.contains_key("task_id"), "missing task_id attr");
    assert_eq!(attrs.get("queue").map(String::as_str), Some(&*queue));
    assert_eq!(
        attrs.get("kind").map(String::as_str),
        Some("e2e_test")
    );
    assert!(attrs.contains_key("attempt"), "missing attempt attr");

    let found = engine
        .find(record.id())
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(found.trace_id(), Some(trace_id_hex));
}

#[tokio::test]
#[serial]
async fn e2e_trace_propagation_across_retries() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let (exporter, provider) = TracerGuard::init();
    let _guard = TracerGuard;

    let queue = common::unique_queue();
    let (engine, token, handle) = boot_trace_engine(pool.clone(), &queue).await;

    let trace_id_hex = "abcdef0123456789abcdef0123456789";

    engine
        .enqueue_raw(
            &queue,
            RetryCountingTask::KIND,
            json!({"succeed_on_attempt": 3}),
            None,
            None,
            Some(3),
            Some(trace_id_hex),
            None,
        )
        .await
        .expect("enqueue");

    common::otel::await_all_terminal(&engine, &queue, 120, Duration::from_millis(200)).await;
    shutdown_engine(token, handle).await;

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "iron_defer.execute")
        .collect();

    assert_eq!(
        exec_spans.len(),
        3,
        "expected 3 execution spans (attempts 1, 2, 3), got {}",
        exec_spans.len()
    );

    let expected_trace_id = TraceId::from_hex(trace_id_hex).unwrap();
    for span in &exec_spans {
        assert_eq!(
            span.span_context.trace_id(),
            expected_trace_id,
            "all retry spans must share the same trace_id (NFR-C3)"
        );
    }

    let span_ids: std::collections::HashSet<_> = exec_spans
        .iter()
        .map(|s| s.span_context.span_id())
        .collect();
    assert_eq!(
        span_ids.len(),
        3,
        "each retry must have a distinct span_id"
    );

    let mut attempts: Vec<i64> = exec_spans
        .iter()
        .filter_map(|s| {
            s.attributes
                .iter()
                .find(|kv| kv.key.as_str() == "attempt")
                .and_then(|kv| match &kv.value {
                    opentelemetry::Value::I64(v) => Some(*v),
                    _ => None,
                })
        })
        .collect();
    attempts.sort();
    assert_eq!(
        attempts,
        vec![1, 2, 3],
        "attempt attribute values must be 1, 2, 3"
    );
}

#[tokio::test]
#[serial]
async fn e2e_no_trace_without_traceparent() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let (exporter, provider) = TracerGuard::init();
    let _guard = TracerGuard;

    let queue = common::unique_queue();
    let (engine, token, handle) = boot_trace_engine(pool.clone(), &queue).await;

    let record = engine
        .enqueue_raw(
            &queue,
            "e2e_test",
            json!({"data": "no-trace"}),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("enqueue");

    common::otel::await_all_terminal(&engine, &queue, 40, Duration::from_millis(100)).await;
    shutdown_engine(token, handle).await;

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "iron_defer.execute")
        .collect();
    assert!(
        exec_spans.is_empty(),
        "expected no iron_defer.execute span when trace_id is None, found {}",
        exec_spans.len()
    );

    let found = engine
        .find(record.id())
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(found.status(), iron_defer::TaskStatus::Completed);
}

#[tokio::test]
#[serial]
async fn e2e_otel_events_emitted_for_transitions() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let (exporter, provider) = TracerGuard::init();
    let _guard = TracerGuard;

    let queue = common::unique_queue();
    let (engine, token, handle) = boot_trace_engine(pool.clone(), &queue).await;

    let trace_id_hex = "11223344556677889900aabbccddeeff";
    engine
        .enqueue_raw(
            &queue,
            "e2e_test",
            json!({"data": "events-test"}),
            None,
            None,
            None,
            Some(trace_id_hex),
            None,
        )
        .await
        .expect("enqueue");

    common::otel::await_all_terminal(&engine, &queue, 40, Duration::from_millis(100)).await;
    shutdown_engine(token, handle).await;

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_span = spans
        .iter()
        .find(|s| s.name == "iron_defer.execute")
        .expect("iron_defer.execute span not found");

    let transition_events: Vec<_> = exec_span
        .events
        .iter()
        .filter(|e| e.name == "task.state_transition")
        .collect();

    assert!(
        transition_events.len() >= 2,
        "expected at least 2 state transition events (pending→running, running→completed), got {}",
        transition_events.len()
    );

    for event in &transition_events {
        let attrs: std::collections::HashMap<String, String> = event
            .attributes
            .iter()
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect();
        assert!(attrs.contains_key("task_id"), "event missing task_id");
        assert!(attrs.contains_key("from_status"), "event missing from_status");
        assert!(attrs.contains_key("to_status"), "event missing to_status");
        assert!(attrs.contains_key("queue"), "event missing queue");
        assert!(attrs.contains_key("kind"), "event missing kind");
        assert!(attrs.contains_key("worker_id"), "event missing worker_id");
    }

    let has_pending_running = transition_events.iter().any(|e| {
        e.attributes.iter().any(|kv| kv.key.as_str() == "from_status" && kv.value.to_string() == "pending")
            && e.attributes.iter().any(|kv| kv.key.as_str() == "to_status" && kv.value.to_string() == "running")
    });
    let has_running_completed = transition_events.iter().any(|e| {
        e.attributes.iter().any(|kv| kv.key.as_str() == "from_status" && kv.value.to_string() == "running")
            && e.attributes.iter().any(|kv| kv.key.as_str() == "to_status" && kv.value.to_string() == "completed")
    });
    assert!(
        has_pending_running,
        "missing pending→running transition event"
    );
    assert!(
        has_running_completed,
        "missing running→completed transition event"
    );
}

#[tokio::test]
#[serial]
async fn e2e_trace_propagation_via_rest_api() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = boot_trace_e2e_server(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let (exporter, provider) = TracerGuard::init();
    let _guard = TracerGuard;

    let trace_id_hex = "deadbeef12345678deadbeef12345678";
    let client = reqwest::Client::new();

    let resp = client
        .post(server.url("/tasks"))
        .header("traceparent", traceparent(trace_id_hex))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_test",
            "payload": {"data": "rest-trace"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 201);
    let post_body: serde_json::Value = resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    e2e::wait_for_status(&client, &server.base_url, task_id, "completed", TIMEOUT).await;

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_span = spans
        .iter()
        .find(|s| s.name == "iron_defer.execute")
        .expect("iron_defer.execute span not found via REST");

    let expected_trace_id = TraceId::from_hex(trace_id_hex).unwrap();
    assert_eq!(
        exec_span.span_context.trace_id(),
        expected_trace_id,
        "REST-submitted trace_id must propagate to execution span"
    );

    server.shutdown().await;
}
