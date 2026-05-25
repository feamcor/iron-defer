//! Observability utilities for the application layer.

use iron_defer_domain::{QueueName, TaskId, TaskKind, TaskStatus, WorkerId};
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
/// All identifier-like fields are validated domain newtypes
/// ([`TaskStatus`], [`QueueName`], [`TaskKind`], [`WorkerId`]). The
/// `trace_id_hex` field stays a raw W3C-format hex string because the domain
/// does not currently expose a `TraceId` newtype — the same shape used by
/// [`iron_defer_domain::TaskRecord::trace_id`].
///
/// `emit` is a no-op when no `trace_id_hex` is set or when it does not parse
/// as a valid 128-bit hex `TraceId` — the event is only recorded when it can
/// be parented to an existing trace.
#[derive(Debug, bon::Builder)]
pub struct StateTransitionEvent {
    task_id: TaskId,
    from_status: TaskStatus,
    to_status: TaskStatus,
    queue: Option<QueueName>,
    kind: Option<TaskKind>,
    #[builder(into)]
    trace_id_hex: Option<String>,
    worker_id: Option<WorkerId>,
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

        let mut span_attrs = vec![KeyValue::new("task_id", self.task_id.to_string())];
        if let Some(queue) = &self.queue {
            span_attrs.push(KeyValue::new("queue", queue.to_string()));
        }
        if let Some(kind) = &self.kind {
            span_attrs.push(KeyValue::new("kind", kind.to_string()));
        }

        let mut span = tracer
            .span_builder("iron_defer.transition")
            .with_kind(SpanKind::Internal)
            .with_attributes(span_attrs)
            .start_with_context(&tracer, &parent);

        let mut event_attrs = vec![
            KeyValue::new("task_id", self.task_id.to_string()),
            KeyValue::new("from_status", self.from_status.as_str()),
            KeyValue::new("to_status", self.to_status.as_str()),
            KeyValue::new("attempt", i64::from(self.attempt)),
        ];
        if let Some(queue) = self.queue {
            event_attrs.push(KeyValue::new("queue", queue.to_string()));
        }
        if let Some(kind) = self.kind {
            event_attrs.push(KeyValue::new("kind", kind.to_string()));
        }
        if let Some(worker) = self.worker_id {
            event_attrs.push(KeyValue::new("worker_id", worker.to_string()));
        }

        span.add_event("task.state_transition", event_attrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_queue() -> QueueName {
        QueueName::try_from("payments").expect("valid queue")
    }

    fn sample_kind() -> TaskKind {
        TaskKind::try_from("charge_card").expect("valid kind")
    }

    /// Builder accepts only the typed newtypes for queue/kind/status —
    /// this is a compile-time guard against the older `String`-based API.
    #[test]
    fn builder_accepts_typed_newtypes() {
        let event = StateTransitionEvent::builder()
            .task_id(TaskId::new())
            .from_status(TaskStatus::Running)
            .to_status(TaskStatus::Pending)
            .queue(sample_queue())
            .kind(sample_kind())
            .worker_id(WorkerId::new())
            .attempt(2)
            .build();
        // No panic on emit even without trace_id (no-op path).
        event.emit();
    }

    /// `emit` with no trace_id is a no-op and does not panic.
    #[test]
    fn emit_without_trace_id_is_noop() {
        StateTransitionEvent::builder()
            .task_id(TaskId::new())
            .from_status(TaskStatus::Running)
            .to_status(TaskStatus::Failed)
            .build()
            .emit();
    }

    /// `emit` with a malformed trace_id hex is a no-op and does not panic.
    #[test]
    fn emit_with_invalid_trace_id_is_noop() {
        StateTransitionEvent::builder()
            .task_id(TaskId::new())
            .from_status(TaskStatus::Running)
            .to_status(TaskStatus::Failed)
            .trace_id_hex("not-hex")
            .build()
            .emit();
    }

    /// `emit` with a valid 32-char hex trace id does not panic, regardless of
    /// whether queue/kind/worker_id are set.
    #[test]
    fn emit_with_valid_trace_id_minimal_fields() {
        StateTransitionEvent::builder()
            .task_id(TaskId::new())
            .from_status(TaskStatus::Suspended)
            .to_status(TaskStatus::Failed)
            .trace_id_hex("4bf92f3577b16b3edb59c6c35e764a39")
            .build()
            .emit();
    }

    #[test]
    fn maybe_setters_accept_options() {
        let queue: Option<QueueName> = Some(sample_queue());
        let kind: Option<TaskKind> = None;
        let trace: Option<String> = Some("4bf92f3577b16b3edb59c6c35e764a39".to_owned());

        StateTransitionEvent::builder()
            .task_id(TaskId::new())
            .from_status(TaskStatus::Running)
            .to_status(TaskStatus::Completed)
            .maybe_queue(queue)
            .maybe_kind(kind)
            .maybe_trace_id_hex(trace)
            .build()
            .emit();
    }
}
