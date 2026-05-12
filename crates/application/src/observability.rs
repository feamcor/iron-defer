//! Observability utilities for the application layer.

use opentelemetry::KeyValue;
use opentelemetry::trace::{
    Span as _, SpanContext, SpanId, SpanKind, TraceContextExt as _, TraceFlags, TraceId,
    TraceState, Tracer as _,
};

/// An OTel event describing a task state transition that occurs outside an
/// active execution span (e.g. manual cancellation, zombie recovery,
/// suspend-timeout expiry).
///
/// Construct with [`StateTransitionEvent::builder`], then call
/// [`StateTransitionEvent::emit`] to fire the event.
///
/// `emit` is a no-op when no `trace_id_hex` is set — the event is only
/// recorded when it can be parented to an existing trace.
#[derive(Debug, bon::Builder)]
pub struct StateTransitionEvent {
    task_id: iron_defer_domain::TaskId,
    #[builder(into)]
    from_status: String,
    #[builder(into)]
    to_status: String,
    #[builder(into)]
    queue: String,
    #[builder(into)]
    kind: String,
    #[builder(into)]
    trace_id_hex: Option<String>,
    worker_id: Option<iron_defer_domain::WorkerId>,
    #[builder(default)]
    attempt: i32,
}

impl StateTransitionEvent {
    /// Emit the event. No-op when no trace id is set or when the trace id is
    /// not a valid hex string.
    pub fn emit(self) {
        let Some(trace_id_hex) = self.trace_id_hex else {
            return;
        };

        let Ok(trace_id) = TraceId::from_hex(&trace_id_hex) else {
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
                KeyValue::new("task_id", self.task_id.to_string()),
                KeyValue::new("queue", self.queue.clone()),
                KeyValue::new("kind", self.kind.clone()),
            ])
            .start_with_context(&tracer, &parent);

        let mut event_attrs = vec![
            KeyValue::new("task_id", self.task_id.to_string()),
            KeyValue::new("from_status", self.from_status),
            KeyValue::new("to_status", self.to_status),
            KeyValue::new("queue", self.queue),
            KeyValue::new("kind", self.kind),
            KeyValue::new("attempt", i64::from(self.attempt)),
        ];
        if let Some(worker) = self.worker_id {
            event_attrs.push(KeyValue::new("worker_id", worker.to_string()));
        }

        span.add_event("task.state_transition", event_attrs);
    }
}
