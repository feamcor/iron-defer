//! Observability utilities for the application layer.

use opentelemetry::KeyValue;
use opentelemetry::trace::{Span as _, SpanContext, SpanId, SpanKind, TraceContextExt as _, TraceFlags, TraceId, TraceState, Tracer as _};

/// Emit an OTel event for a task state transition that occurs outside an active
/// execution span (e.g. manual cancellation, zombie recovery).
///
/// If a `trace_id` is present, this creates a short-lived span parented to that
/// trace, adds the event, and ends. If no `trace_id` is present, it is a no-op.
#[allow(clippy::too_many_arguments)]
pub fn emit_otel_state_transition(
    trace_id_hex: Option<&str>,
    task_id: iron_defer_domain::TaskId,
    from_status: &str,
    to_status: &str,
    queue: &str,
    kind: &str,
    worker_id: Option<iron_defer_domain::WorkerId>,
    attempt: i32,
) {
    let Some(trace_id_hex) = trace_id_hex else {
        return;
    };

    let Ok(trace_id) = TraceId::from_hex(trace_id_hex) else {
        return;
    };

    let remote_ctx = SpanContext::new(
        trace_id,
        SpanId::INVALID,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let parent = opentelemetry::Context::new().with_remote_span_context(remote_ctx);
    let tracer = opentelemetry::global::tracer("iron-defer");

    let mut span = tracer
        .span_builder("iron_defer.transition")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new("task_id", task_id.to_string()),
            KeyValue::new("queue", queue.to_owned()),
            KeyValue::new("kind", kind.to_owned()),
        ])
        .start_with_context(&tracer, &parent);

    let mut event_attrs = vec![
        KeyValue::new("task_id", task_id.to_string()),
        KeyValue::new("from_status", from_status.to_owned()),
        KeyValue::new("to_status", to_status.to_owned()),
        KeyValue::new("queue", queue.to_owned()),
        KeyValue::new("kind", kind.to_owned()),
        KeyValue::new("attempt", i64::from(attempt)),
    ];
    if let Some(worker) = worker_id {
        event_attrs.push(KeyValue::new("worker_id", worker.to_string()));
    }

    span.add_event("task.state_transition", event_attrs);
}
