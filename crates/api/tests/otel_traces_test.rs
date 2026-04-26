mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{
    IronDefer, Task, TaskContext, TaskError, WorkerConfig,
};
use opentelemetry::global;
use opentelemetry::trace::TraceId;
use opentelemetry_sdk::testing::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::TracerProvider;
use serde::{Deserialize, Serialize};
use serial_test::serial;
use tokio_util::sync::CancellationToken;

use common::otel::build_harness;

#[derive(Debug, Serialize, Deserialize)]
struct TraceEcho;

impl Task for TraceEcho {
    const KIND: &'static str = "trace_echo";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TraceFailOnce {
    should_fail: bool,
}

impl Task for TraceFailOnce {
    const KIND: &'static str = "trace_fail_once";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        if self.should_fail {
            Err(TaskError::ExecutionFailed {
                kind: iron_defer::ExecutionErrorKind::HandlerFailed {
                    source: "intentional failure for trace test".into(),
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
        shutdown_timeout: Duration::from_secs(2),
        ..WorkerConfig::default()
    }
}

fn sample_trace_id() -> String {
    "4bf92f3577b16b3edb59c6c35e764a39".to_owned()
}

async fn build_engine(pool: sqlx::PgPool, queue: &str) -> Arc<IronDefer> {
    let harness = build_harness();
    let engine = IronDefer::builder()
        .pool(pool)
        .register::<TraceEcho>()
        .register::<TraceFailOnce>()
        .worker_config(fast_worker_config())
        .queue(queue)
        .metrics(harness.metrics)
        .prometheus_registry(harness.registry)
        .build()
        .await
        .expect("engine build");
    Arc::new(engine)
}

#[tokio::test]
async fn trace_id_persisted_in_database() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Docker");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), &queue).await;

    let trace_id = sample_trace_id();

    let record = engine
        .enqueue_raw(
            &queue,
            "trace_echo",
            serde_json::json!({}),
            None,
            None,
            None,
            Some(&trace_id),
            None,
        )
        .await
        .expect("enqueue with trace_id");

    assert_eq!(record.trace_id(), Some(trace_id.as_str()));

    let found = engine.find(record.id()).await.expect("find").expect("exists");
    assert_eq!(found.trace_id(), Some(trace_id.as_str()));
}

#[tokio::test]
async fn no_trace_id_when_not_provided() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Docker");
        return;
    };

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), &queue).await;

    let record = engine
        .enqueue_raw(&queue, "trace_echo", serde_json::json!({}), None, None, None, None, None)
        .await
        .expect("enqueue without trace_id");

    assert_eq!(record.trace_id(), None);
}

#[tokio::test]
#[serial]
async fn span_created_with_correct_trace_id_and_attributes() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Docker");
        return;
    };

    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = global::set_tracer_provider(provider.clone());

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), &queue).await;

    let trace_id = sample_trace_id();

    engine
        .enqueue_raw(
            &queue,
            "trace_echo",
            serde_json::json!({}),
            None,
            None,
            None,
            Some(&trace_id),
            None,
        )
        .await
        .expect("enqueue");

    let token = CancellationToken::new();
    let cancel = token.clone();
    let eng = engine.clone();
    let worker_handle = tokio::spawn(async move {
        eng.start(cancel).await.expect("engine start");
    });

    common::otel::await_all_terminal(&engine, &queue, 40, Duration::from_millis(100)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker_handle)
        .await
        .expect("worker exit")
        .expect("worker ok");

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_span = spans
        .iter()
        .find(|s| s.name == "iron_defer.execute")
        .expect("iron_defer.execute span not found");

    let expected_trace_id = TraceId::from_hex(&trace_id).unwrap();
    assert_eq!(exec_span.span_context.trace_id(), expected_trace_id);

    let attrs: std::collections::HashMap<String, String> = exec_span
        .attributes
        .iter()
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect();
    assert!(attrs.contains_key("task_id"), "missing task_id attr");
    assert_eq!(attrs.get("queue").map(String::as_str), Some(&*queue));
    assert_eq!(attrs.get("kind").map(String::as_str), Some("trace_echo"));
    assert!(attrs.contains_key("attempt"), "missing attempt attr");

    let transition_events: Vec<_> = exec_span.events
        .iter()
        .filter(|e| e.name == "task.state_transition")
        .collect();
    assert!(
        transition_events.len() >= 2,
        "expected at least 2 state transition events (pending→running, running→completed), got {}",
        transition_events.len()
    );
}

#[tokio::test]
#[serial]
async fn no_span_created_without_trace_id() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Docker");
        return;
    };

    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = global::set_tracer_provider(provider.clone());

    let queue = common::unique_queue();
    let engine = build_engine(pool.clone(), &queue).await;

    engine
        .enqueue_raw(&queue, "trace_echo", serde_json::json!({}), None, None, None, None, None)
        .await
        .expect("enqueue");

    let token = CancellationToken::new();
    let cancel = token.clone();
    let eng = engine.clone();
    let worker_handle = tokio::spawn(async move {
        eng.start(cancel).await.expect("engine start");
    });

    common::otel::await_all_terminal(&engine, &queue, 40, Duration::from_millis(100)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker_handle)
        .await
        .expect("worker exit")
        .expect("worker ok");

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
}

#[tokio::test]
#[serial]
async fn retry_spans_share_same_trace_id() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] no Docker");
        return;
    };

    let exporter = InMemorySpanExporter::default();
    let provider = TracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let _prev = global::set_tracer_provider(provider.clone());

    let queue = common::unique_queue();

    let harness = build_harness();
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<TraceEcho>()
        .register::<TraceFailOnce>()
        .worker_config(WorkerConfig {
            concurrency: 1,
            poll_interval: Duration::from_millis(50),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .queue(&queue)
        .metrics(harness.metrics)
        .prometheus_registry(harness.registry)
        .build()
        .await
        .expect("engine build");
    let engine = Arc::new(engine);

    let trace_id = sample_trace_id();

    engine
        .enqueue_raw(
            &queue,
            "trace_fail_once",
            serde_json::json!({"should_fail": true}),
            None,
            None,
            Some(3),
            Some(&trace_id),
            None,
        )
        .await
        .expect("enqueue");

    let token = CancellationToken::new();
    let cancel = token.clone();
    let eng = engine.clone();
    let worker_handle = tokio::spawn(async move {
        eng.start(cancel).await.expect("engine start");
    });

    common::otel::await_all_terminal(&engine, &queue, 80, Duration::from_millis(200)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker_handle)
        .await
        .expect("worker exit")
        .expect("worker ok");

    provider.force_flush();
    let spans = exporter.get_finished_spans().expect("get spans");

    let exec_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "iron_defer.execute")
        .collect();

    assert!(
        exec_spans.len() >= 2,
        "expected at least 2 retry spans (NFR-C3), got {}",
        exec_spans.len()
    );

    let expected_trace_id = TraceId::from_hex(&trace_id).unwrap();
    for span in &exec_spans {
        assert_eq!(
            span.span_context.trace_id(),
            expected_trace_id,
            "all retry spans must share the same trace_id"
        );
    }
}
