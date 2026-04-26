//! Worker pool service — claims and executes tasks with bounded concurrency.
//!
//! Architecture references:
//! - §D2.2: `JoinSet` + `Semaphore` concurrency model
//! - §D2.3: Interval-based polling (default 500ms)
//! - §C2: `CancellationToken` polled BETWEEN tasks only — once claimed, a
//!   task runs to completion. NEVER `tokio::select!` against the token
//!   during execution.
//! - §Process Patterns (Tracing instrumentation): `#[instrument]` conventions

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::metrics::Metrics;
use iron_defer_domain::{
    CheckpointWriter, ExecutionErrorKind, QueueName, TaskContext, TaskError, TaskRecord,
    TaskStatus, WorkerId,
};
use opentelemetry::KeyValue;
use opentelemetry::trace::{
    Span as _, SpanContext, SpanId, SpanKind, TraceContextExt, TraceFlags, TraceId, TraceState,
    Tracer,
};
use rand::Rng;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, error, info, instrument, warn};

use crate::config::WorkerConfig;
use crate::ports::TaskRepository;
use crate::registry::TaskRegistry;

/// Classifier that inspects a `TaskError` and returns `true` when the
/// underlying cause is a pool-acquire saturation (e.g. `sqlx::Error::PoolTimedOut`).
///
/// The application layer does not depend on `sqlx`, so the real classifier
/// is wired in from the infrastructure crate at `IronDefer::start()` time
/// (see `iron_defer_infrastructure::is_pool_timeout`). Tests and default
/// construction use a no-op that always returns `false`.
///
/// # Safety contract
///
/// The classifier **must not panic**. A panic inside the closure will
/// unwind through the poll loop and terminate the worker. All in-crate
/// classifiers (`is_pool_timeout`) are non-panicking by construction.
pub type SaturationClassifier = Arc<dyn Fn(&TaskError) -> bool + Send + Sync>;

/// Async worker pool that continuously claims and executes tasks.
///
/// Constructed by `IronDefer::start()` in the api crate, or directly in tests.
/// Polls a single queue on `config.poll_interval` ticks, using a `Semaphore`
/// for bounded concurrency and a `JoinSet` for in-flight task tracking.
#[derive(bon::Builder)]
pub struct WorkerService {
    repo: Arc<dyn TaskRepository>,
    registry: Arc<TaskRegistry>,
    config: WorkerConfig,
    queue: QueueName,
    token: CancellationToken,
    worker_id: WorkerId,
    #[builder(default = Arc::new(|_| false))]
    is_saturation: SaturationClassifier,
    metrics: Option<Metrics>,
    #[builder(default = Arc::new(AtomicU32::new(0)))]
    active_tasks: Arc<AtomicU32>,
    checkpoint_writer: Option<Arc<dyn CheckpointWriter>>,
}

impl WorkerService {
    /// The worker identity used for claiming tasks and lease tracking.
    #[must_use]
    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    /// Run the poll loop until the cancellation token fires, then drain
    /// in-flight tasks to completion.
    ///
    /// This is the simple entry point that combines polling and draining.
    /// For shutdown timeout control, use [`run_poll_loop`](Self::run_poll_loop)
    /// to get the `JoinSet` back and drain externally with a timeout.
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if concurrency is zero.
    #[instrument(
        skip(self),
        fields(queue = %self.queue, concurrency = %self.config.concurrency),
        err
    )]
    pub async fn run(&self) -> Result<(), TaskError> {
        let mut join_set = self.run_poll_loop().await?;
        drain_join_set(&mut join_set).await;
        Ok(())
    }

    /// Run the poll loop until the cancellation token fires, returning the
    /// `JoinSet` of in-flight task handles for external drain control.
    ///
    /// The caller is responsible for draining the returned `JoinSet`. This
    /// enables wrapping the drain in a `tokio::time::timeout` for graceful
    /// shutdown with lease release on timeout (Architecture D6.1).
    ///
    /// # Errors
    ///
    /// Returns `TaskError::InvalidPayload` if concurrency is zero.
    #[allow(clippy::too_many_lines)]
    pub async fn run_poll_loop(&self) -> Result<JoinSet<()>, TaskError> {
        if self.config.concurrency == 0 {
            return Err(TaskError::InvalidPayload {
                kind: iron_defer_domain::PayloadErrorKind::Validation {
                    message: "worker concurrency must be >= 1".to_string(),
                },
            });
        }

        let worker_id = self.worker_id;
        let semaphore = Arc::new(Semaphore::new(self.config.concurrency as usize));
        let mut join_set: JoinSet<()> = JoinSet::new();
        let mut tick = interval(self.config.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let max_claim_backoff = self.config.max_claim_backoff;
        let queue_str: Arc<str> = self.queue.to_string().into();

        let mut consecutive_errors: u32 = 0;
        let mut backoff_until: Option<tokio::time::Instant> = None;

        info!(
            worker_id = %worker_id,
            queue = %self.queue,
            "worker started"
        );

        loop {
            if let Some(deadline) = backoff_until.take() {
                tokio::select! {
                    () = self.token.cancelled() => {
                        info!("cancellation received during backoff");
                        break;
                    }
                    _ = join_set.join_next(), if !join_set.is_empty() => {
                        // Reap tasks during backoff to release resources
                    }
                    () = tokio::time::sleep_until(deadline) => {}
                }
            }

            tokio::select! {
                () = self.token.cancelled() => {
                    info!("cancellation received, returning in-flight handles for drain");
                    break;
                }
                _ = tick.tick() => {
                    while join_set.try_join_next().is_some() {}

                    let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                        continue;
                    };

                    let claim_result = tokio::select! {
                        () = self.token.cancelled() => {
                            drop(permit);
                            info!("cancellation received during claim attempt");
                            break;
                        }
                        result = self.repo.claim_next(&self.queue, worker_id, self.config.lease_duration, self.config.region.as_deref()) => {
                            result
                        }
                    };

                    match claim_result {
                        Ok(Some(task)) => {
                            consecutive_errors = 0;

                            // Check token between claim and spawn to avoid
                            // orphaning a Running task when shutdown fires
                            // during this window. The task was claimed
                            // (attempts incremented by claim_next) but never
                            // dispatched — release its lease immediately.
                            // Note: release_leases_for_worker increments
                            // attempts again, so a claimed-but-never-dispatched
                            // task consumes 2 attempt slots.
                            if self.token.is_cancelled() {
                                info!(
                                    event = "claim_cancelled",
                                    task_id = %task.id(),
                                    worker_id = %worker_id,
                                    "cancellation detected after claim, releasing lease"
                                );
                                // claim succeeded but shutdown detected — release back to pending immediately
                                if let Ok(Some(trace_id)) = self.repo.release_lease_for_task(task.id()).await {
                                    crate::emit_otel_state_transition(
                                        Some(&trace_id),
                                        task.id(),
                                        "running",
                                        "pending",
                                        task.queue().as_str(),
                                        task.kind().as_str(),
                                        Some(worker_id),
                                        task.attempts().get(),
                                    );
                                }
                                drop(permit);
                                break;
                            }

                            if self.config.log_payload {
                                info!(
                                    event = "task_claimed",
                                    task_id = %task.id(),
                                    queue = %self.queue,
                                    worker_id = %worker_id,
                                    kind = %task.kind(),
                                    attempt = %task.attempts(),
                                    payload = ?task.payload(),
                                    "task claimed"
                                );
                            } else {
                                info!(
                                    event = "task_claimed",
                                    task_id = %task.id(),
                                    queue = %self.queue,
                                    worker_id = %worker_id,
                                    kind = %task.kind(),
                                    attempt = %task.attempts(),
                                    "task claimed"
                                );
                            }

                            if let Some(ref m) = self.metrics {
                                let region_label = task.region().unwrap_or("global");
                                let labels = &[
                                    KeyValue::new("queue", queue_str.clone()),
                                    KeyValue::new("kind", task.kind().as_str().to_owned()),
                                    KeyValue::new("region", region_label.to_owned()),
                                ];
                                m.task_attempts_total.add(1, labels);
                            }

                            let ctx = DispatchContext {
                                repo: self.repo.clone(),
                                registry: self.registry.clone(),
                                worker_id,
                                base_delay_secs: self.config.base_delay.as_secs_f64(),
                                max_delay_secs: self.config.max_delay.as_secs_f64(),
                                log_payload: self.config.log_payload,
                                metrics: self.metrics.clone(),
                                queue_str: queue_str.clone(),
                                checkpoint_writer: self.checkpoint_writer.clone(),
                            };

                            let active_task_guard = ActiveTaskGuard::new(
                                self.active_tasks.clone(),
                                self.metrics.clone(),
                                self.config.concurrency,
                                queue_str.clone(),
                            );

                            join_set.spawn(
                                async move {
                                    let _active_task_guard = active_task_guard;
                                    dispatch_task(task, &ctx).await;
                                    drop(permit);
                                }
                                .in_current_span(),
                            );
                        }
                        Ok(None) => {
                            consecutive_errors = 0;
                            drop(permit);
                        }
                        Err(e) => {
                            // Backoff calculation: poll_interval * 2^(consecutive_errors-1)
                            // We use saturating math to avoid overflow.
                            // First error (consecutive_errors=0) starts at 1× poll_interval.
                            let base_delay = self.config.poll_interval.saturating_mul(
                                2u32.saturating_pow(consecutive_errors.saturating_sub(1))
                            );

                            // Cap base_delay first, then calculate jitter against remaining budget
                            // so total never exceeds max_claim_backoff.
                            let capped_base = base_delay.min(max_claim_backoff);
                            let jitter_budget = max_claim_backoff.saturating_sub(capped_base);
                            let jitter_range = u64::try_from(jitter_budget.as_millis()).unwrap_or(0).min(u64::MAX - 1);
                            let jitter_ms = if jitter_range > 0 {
                                rand::rng().random_range(0..=jitter_range)
                            } else {
                                0
                            };

                            let delay = capped_base
                                .saturating_add(Duration::from_millis(jitter_ms));

                            consecutive_errors = consecutive_errors.saturating_add(1);
                            let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);

                            let is_sat = (self.is_saturation)(&e);

                            if is_sat {
                                warn!(
                                    event = "pool_saturated",
                                    worker_id = %worker_id,
                                    queue = %self.queue,
                                    error = %e,
                                    consecutive_errors = consecutive_errors,
                                    backoff_ms = delay_ms,
                                    "postgres connection pool saturated — backing off"
                                );
                            } else {
                                warn!(
                                    event = "claim_backoff",
                                    worker_id = %worker_id,
                                    queue = %self.queue,
                                    error = %e,
                                    consecutive_errors = consecutive_errors,
                                    backoff_ms = delay_ms,
                                    "failed to claim task — backing off"
                                );
                            }

                            if let Some(ref m) = self.metrics {
                                m.claim_backoff_total.add(1, &[
                                    KeyValue::new("queue", queue_str.to_string()),
                                    KeyValue::new("saturation", if is_sat { "true" } else { "false" }),
                                ]);
                                m.claim_backoff_seconds.record(delay.as_secs_f64(), &[
                                    KeyValue::new("queue", queue_str.to_string()),
                                ]);
                            }

                            let now = tokio::time::Instant::now();
                            backoff_until = Some(
                                now.checked_add(delay)
                                    .or_else(|| now.checked_add(max_claim_backoff))
                                    .unwrap_or(now),
                            );
                            drop(permit);
                        }
                    }
                }
            }
        }

        Ok(join_set)
    }
}

/// RAII guard that pairs an `active_tasks` counter increment with its
/// decrement across an arbitrary number of early-return / panic / abort
/// paths.
///
/// This guard keeps the in-flight counter accurate even when spawned tasks are
/// cancelled or unwind early, because decrement happens in `Drop`.
struct ActiveTaskGuard {
    counter: Arc<AtomicU32>,
    metrics: Option<Metrics>,
    concurrency: u32,
    queue: Arc<str>,
}

impl ActiveTaskGuard {
    fn new(
        counter: Arc<AtomicU32>,
        metrics: Option<Metrics>,
        concurrency: u32,
        queue: Arc<str>,
    ) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self::record_utilization(&counter, metrics.as_ref(), concurrency, &queue);
        Self {
            counter,
            metrics,
            concurrency,
            queue,
        }
    }

    fn record_utilization(
        counter: &Arc<AtomicU32>,
        metrics: Option<&Metrics>,
        concurrency: u32,
        queue: &str,
    ) {
        if let Some(m) = metrics {
            let ratio = f64::from(counter.load(Ordering::Relaxed)) / f64::from(concurrency.max(1));
            m.worker_pool_utilization
                .record(ratio, &[KeyValue::new("queue", queue.to_owned())]);
        }
    }
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
        Self::record_utilization(
            &self.counter,
            self.metrics.as_ref(),
            self.concurrency,
            &self.queue,
        );
    }
}

/// Drain all remaining handles from a `JoinSet`, logging any panics.
///
/// Used after the poll loop exits to wait for in-flight tasks to complete.
pub async fn drain_join_set(join_set: &mut JoinSet<()>) {
    while let Some(result) = join_set.join_next().await {
        if let Err(e) = result {
            error!(error = %e, "in-flight task panicked during drain");
        }
    }
    info!("worker pool stopped, all in-flight tasks drained");
}

#[derive(Clone)]
struct DispatchContext {
    repo: Arc<dyn TaskRepository>,
    registry: Arc<TaskRegistry>,
    worker_id: WorkerId,
    base_delay_secs: f64,
    max_delay_secs: f64,
    log_payload: bool,
    metrics: Option<Metrics>,
    queue_str: Arc<str>,
    checkpoint_writer: Option<Arc<dyn CheckpointWriter>>,
}

/// Dispatch a single claimed task through the registry.
///
/// Looks up the handler by `task.kind`, builds `TaskContext`, and calls the
/// handler. On success calls `complete`, on failure calls `fail`. If no
/// handler is registered, the task is failed (not panicked) — see story
/// 1B.2 architecture decision.
///
/// `log_payload`: when `true`, every lifecycle log
/// record (`task_completed`, `task_failed_retry`, `task_failed_terminal`)
/// carries a `payload = ?task.payload` field. Defaults to `false` via
/// `WorkerConfig::default()` so privacy-by-default (FR38) is preserved
/// unless the operator explicitly opts in.
#[allow(clippy::too_many_lines)]
async fn dispatch_task(task: TaskRecord, ctx: &DispatchContext) {
    // Capture at the top of the function so duration_ms includes registry
    // lookup + handler execution + complete/fail round-trip. DB timestamps
    // are not a safe substitute — clock skew between app host and DB would
    // invalidate the signal.
    let started = std::time::Instant::now();
    let mut task_ctx = TaskContext::new(task.id(), ctx.worker_id, task.attempts());
    if let Some(ref writer) = ctx.checkpoint_writer {
        task_ctx = task_ctx.with_checkpoint(task.checkpoint().cloned(), writer.clone());
    }
    task_ctx = task_ctx.with_signal_payload(task.signal_payload().cloned());

    let Some(handler) = ctx.registry.get(task.kind().as_str()) else {
        // Emit a canonical lifecycle event
        // for this failure site (FR19 — every transition is a log). The
        // task is about to transition to Failed via repo.fail below.
        //
        // Emit the auxiliary
        // `task_fail_storage_error` FIRST (categorizes failure class)
        // and the canonical `task_failed_retry` / `task_failed_terminal`
        // SECOND (restores FR19 pairing with `task_claimed`). When
        // `repo.fail` itself fails, only the auxiliary event is emitted
        // — there is no `record` to base the lifecycle event on.
        let msg = format!("no handler registered for kind: {:?}", task.kind());
        error!(task_id = %task.id(), kind = %task.kind(), "{}", msg);
        emit_task_fail_storage_error(&task, ctx.worker_id, &msg, ctx.log_payload);
        let err = TaskError::ExecutionFailed {
            kind: ExecutionErrorKind::MissingHandler {
                kind: task.kind().as_str().to_string(),
            },
        };
        match ctx
            .repo
            .fail(
                task.id(),
                &err.to_string(),
                ctx.base_delay_secs,
                ctx.max_delay_secs,
            )
            .await
        {
            Ok(record) => {
                emit_task_failed(
                    &task,
                    ctx.worker_id,
                    &record,
                    &err.to_string(),
                    ctx.log_payload,
                );
                // Missing-handler is a
                // configuration error, not a handler-execution failure.
                // `task_failures_total` tracks handler outcomes (Err /
                // panic) so that failures/attempts is a meaningful
                // reliability signal. Missing-handler events surface
                // through the `task_fail_storage_error` log and the
                // `task_attempts_total` counter already incremented at
                // claim time — no metric emission here.
            }
            Err(e) => {
                error!(task_id = %task.id(), error = %e, "failed to record task failure for missing handler");
            }
        }
        return;
    };

    let otel_ctx: Option<opentelemetry::Context> = task.trace_id().and_then(|trace_id_hex| {
        let trace_id = match TraceId::from_hex(trace_id_hex) {
            Ok(id) => id,
            Err(_) => return None,
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
            .span_builder("iron_defer.execute")
            .with_kind(SpanKind::Consumer)
            .with_attributes(vec![
                KeyValue::new("task_id", task.id().to_string()),
                KeyValue::new("queue", task.queue().to_string()),
                KeyValue::new("kind", task.kind().to_string()),
                KeyValue::new("attempt", i64::from(task.attempts().get())),
            ])
            .start_with_context(&tracer, &parent);

        span.add_event(
            "task.state_transition",
            vec![
                KeyValue::new("task_id", task.id().to_string()),
                KeyValue::new("from_status", "pending"),
                KeyValue::new("to_status", "running"),
                KeyValue::new("queue", task.queue().to_string()),
                KeyValue::new("kind", task.kind().to_string()),
                KeyValue::new("worker_id", ctx.worker_id.to_string()),
                KeyValue::new("attempt", i64::from(task.attempts().get())),
            ],
        );
        Some(opentelemetry::Context::current().with_span(span))
    });

    let handler_clone = handler.clone();
    let payload = task.payload_arc().clone();
    let task_ctx_clone = task_ctx.clone();

    let join_handle = {
        let spawned_ctx = otel_ctx.clone();
        tokio::spawn(async move {
            if let Some(otel_context) = spawned_ctx {
                use opentelemetry::trace::FutureExt as _;
                handler_clone
                    .execute(&payload, &task_ctx_clone)
                    .with_context(otel_context)
                    .await
            } else {
                handler_clone.execute(&payload, &task_ctx_clone).await
            }
        })
    };

    let handler_result = match join_handle.await {
        Ok(inner) => inner,
        Err(join_err) if join_err.is_panic() => {
            let panic_msg = extract_panic_message(join_err);

            // Emit transition event before returning from panic
            if let Some(ref otel_context) = otel_ctx {
                let to_status = if task.attempts().get() < task.max_attempts().get() {
                    "pending"
                } else {
                    "failed"
                };
                otel_context.span().add_event(
                    "task.state_transition",
                    vec![
                        KeyValue::new("task_id", task.id().to_string()),
                        KeyValue::new("from_status", "running"),
                        KeyValue::new("to_status", to_status),
                        KeyValue::new("queue", task.queue().to_string()),
                        KeyValue::new("kind", task.kind().to_string()),
                        KeyValue::new("worker_id", ctx.worker_id.to_string()),
                        KeyValue::new("attempt", i64::from(task.attempts().get())),
                        KeyValue::new("error", "panic"),
                    ],
                );
            }

            emit_task_fail_panic(&task, ctx.worker_id, &panic_msg, ctx.log_payload);
            let err = TaskError::ExecutionFailed {
                kind: ExecutionErrorKind::HandlerPanicked {
                    message: panic_msg.clone(),
                },
            };
            match ctx
                .repo
                .fail(
                    task.id(),
                    &err.to_string(),
                    ctx.base_delay_secs,
                    ctx.max_delay_secs,
                )
                .await
            {
                Ok(record) => {
                    emit_task_failed(
                        &task,
                        ctx.worker_id,
                        &record,
                        &err.to_string(),
                        ctx.log_payload,
                    );
                    if let Some(ref m) = ctx.metrics {
                        m.task_failures_total.add(
                            1,
                            &[
                                KeyValue::new("queue", ctx.queue_str.clone()),
                                KeyValue::new("kind", task.kind().as_str().to_owned()),
                            ],
                        );
                    }
                }
                Err(e) => {
                    error!(task_id = %task.id(), error = %e, "failed to record task panic in repo");
                }
            }
            return;
        }
        Err(join_err) => {
            // Cancellation (not a panic) — no canonical lifecycle event:
            // the token cancellation is an operator-initiated shutdown,
            // not an FR19 task-state transition.
            if let Some(ref otel_context) = otel_ctx {
                otel_context.span().add_event(
                    "task.state_transition",
                    vec![
                        KeyValue::new("task_id", task.id().to_string()),
                        KeyValue::new("from_status", "running"),
                        KeyValue::new("to_status", "pending"),
                        KeyValue::new("queue", task.queue().to_string()),
                        KeyValue::new("kind", task.kind().to_string()),
                        KeyValue::new("worker_id", ctx.worker_id.to_string()),
                        KeyValue::new("attempt", i64::from(task.attempts().get())),
                        KeyValue::new("reason", "shutdown"),
                    ],
                );
            }
            error!(task_id = %task.id(), error = %join_err, "handler task join error");
            return;
        }
    };

    // Capture the elapsed duration once at the branch entry so `duration_ms`
    // (log) and `elapsed_secs` (metric) are derived from the same
    // `Instant::elapsed` read. Two reads
    // could disagree under load because the second call samples a later
    // clock value than the first.
    let elapsed = started.elapsed();
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let elapsed_secs = elapsed.as_secs_f64();

    match handler_result {
        Ok(()) => match ctx.repo.complete(task.id()).await {
            Ok(_) => {
                if let Some(ref otel_context) = otel_ctx {
                    otel_context.span().add_event(
                        "task.state_transition",
                        vec![
                            KeyValue::new("task_id", task.id().to_string()),
                            KeyValue::new("from_status", "running"),
                            KeyValue::new("to_status", "completed"),
                            KeyValue::new("queue", task.queue().to_string()),
                            KeyValue::new("kind", task.kind().to_string()),
                            KeyValue::new("worker_id", ctx.worker_id.to_string()),
                            KeyValue::new("attempt", i64::from(task.attempts().get())),
                        ],
                    );
                }
                emit_task_completed(&task, ctx.worker_id, duration_ms, ctx.log_payload);
                if let Some(ref m) = ctx.metrics {
                    let region_label = task.region().unwrap_or("global");
                    let labels = &[
                        KeyValue::new("queue", ctx.queue_str.clone()),
                        KeyValue::new("kind", task.kind().as_str().to_owned()),
                        KeyValue::new("status", "completed"),
                        KeyValue::new("region", region_label.to_owned()),
                    ];
                    m.task_duration_seconds.record(elapsed_secs, labels);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                error!(task_id = %task.id(), error = %e, "failed to mark task as completed");
                emit_task_fail_storage_error(&task, ctx.worker_id, &msg, ctx.log_payload);
                if let Some(ref m) = ctx.metrics {
                    m.task_duration_seconds.record(
                        elapsed_secs,
                        &[
                            KeyValue::new("queue", ctx.queue_str.clone()),
                            KeyValue::new("kind", task.kind().as_str().to_owned()),
                            KeyValue::new("status", "storage_error"),
                        ],
                    );
                }
            }
        },
        Err(TaskError::SuspendRequested) => match ctx.repo.suspend(task.id()).await {
            Ok(_record) => {
                if let Some(ref otel_context) = otel_ctx {
                    otel_context.span().add_event(
                        "task.state_transition",
                        vec![
                            KeyValue::new("task_id", task.id().to_string()),
                            KeyValue::new("from_status", "running"),
                            KeyValue::new("to_status", "suspended"),
                            KeyValue::new("queue", task.queue().to_string()),
                            KeyValue::new("kind", task.kind().to_string()),
                            KeyValue::new("worker_id", ctx.worker_id.to_string()),
                            KeyValue::new("attempt", i64::from(task.attempts().get())),
                        ],
                    );
                }
                emit_task_suspended(&task, ctx.worker_id, ctx.log_payload);
                if let Some(ref m) = ctx.metrics {
                    m.tasks_suspended_total.add(
                        1,
                        &[
                            KeyValue::new("queue", ctx.queue_str.clone()),
                            KeyValue::new("kind", task.kind().as_str().to_owned()),
                        ],
                    );
                    m.task_duration_seconds.record(
                        elapsed_secs,
                        &[
                            KeyValue::new("queue", ctx.queue_str.clone()),
                            KeyValue::new("kind", task.kind().as_str().to_owned()),
                            KeyValue::new("status", "suspended"),
                        ],
                    );
                }
            }
            Err(e) => {
                let msg = e.to_string();
                error!(task_id = %task.id(), error = %e, "failed to suspend task");
                emit_task_fail_storage_error(&task, ctx.worker_id, &msg, ctx.log_payload);
            }
        },
        Err(e) => {
            if let Some(ref otel_context) = otel_ctx {
                let to_status = if task.attempts().get() < task.max_attempts().get() {
                    "pending"
                } else {
                    "failed"
                };
                otel_context.span().add_event(
                    "task.state_transition",
                    vec![
                        KeyValue::new("task_id", task.id().to_string()),
                        KeyValue::new("from_status", "running"),
                        KeyValue::new("to_status", to_status),
                        KeyValue::new("queue", task.queue().to_string()),
                        KeyValue::new("kind", task.kind().to_string()),
                        KeyValue::new("worker_id", ctx.worker_id.to_string()),
                        KeyValue::new("attempt", i64::from(task.attempts().get())),
                        KeyValue::new("error", e.to_string()),
                    ],
                );
            }
            handle_task_failure(&task, ctx, &e, elapsed_secs).await;
        }
    }
}

/// Extract a human-readable message from a panicked `JoinError`.
///
/// The panic payload is `Box<dyn Any>`. `panic!("msg")` → `&'static str`;
/// `panic!("{}", x)` → `String`; `panic_any(value)` → arbitrary. We try
/// the two common shapes and fall back to a fixed string otherwise.
fn extract_panic_message(join_err: tokio::task::JoinError) -> String {
    match join_err.try_into_panic() {
        Ok(payload) => {
            if let Some(s) = payload.downcast_ref::<&'static str>() {
                return format!("handler panicked: {s}");
            }
            if let Some(s) = payload.downcast_ref::<String>() {
                return format!("handler panicked: {s}");
            }
            if let Some(s) = payload.downcast_ref::<Box<String>>() {
                return format!("handler panicked: {s}");
            }
            format!(
                "handler panicked (payload type: {:?})",
                (*payload).type_id()
            )
        }
        Err(_) => "handler panicked (payload unavailable)".to_string(),
    }
}

/// Emit the `task_completed` lifecycle log (FR19) with payload gated on
/// `log_payload` (FR38 / FR39).
fn emit_task_completed(
    task: &TaskRecord,
    worker_id: WorkerId,
    duration_ms: u64,
    log_payload: bool,
) {
    let region = task.region();
    if log_payload {
        info!(
            event = "task_completed",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            region,
            duration_ms = duration_ms,
            payload = ?task.payload(),
            "task completed"
        );
    } else {
        info!(
            event = "task_completed",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            region,
            duration_ms = duration_ms,
            "task completed"
        );
    }
}

fn emit_task_suspended(task: &TaskRecord, worker_id: WorkerId, log_payload: bool) {
    if log_payload {
        info!(
            event = "task_suspended",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            payload = ?task.payload(),
            "task suspended"
        );
    } else {
        info!(
            event = "task_suspended",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            "task suspended"
        );
    }
}

/// Record task failure in the repository and emit the appropriate lifecycle
/// log (`task_failed_retry` or `task_failed_terminal`).
///
/// `elapsed_secs` is the single elapsed-time reading captured by
/// `dispatch_task` so log and metric values
/// agree.
async fn handle_task_failure(
    task: &TaskRecord,
    ctx: &DispatchContext,
    err: &TaskError,
    elapsed_secs: f64,
) {
    let error_message = err.to_string();
    match ctx
        .repo
        .fail(
            task.id(),
            &error_message,
            ctx.base_delay_secs,
            ctx.max_delay_secs,
        )
        .await
    {
        Ok(record) => {
            emit_task_failed(
                task,
                ctx.worker_id,
                &record,
                &error_message,
                ctx.log_payload,
            );
            if let Some(ref m) = ctx.metrics {
                let labels = &[
                    KeyValue::new("queue", ctx.queue_str.clone()),
                    KeyValue::new("kind", task.kind().as_str().to_owned()),
                ];
                m.task_failures_total.add(1, labels);
                m.task_duration_seconds.record(
                    elapsed_secs,
                    &[
                        KeyValue::new("queue", ctx.queue_str.clone()),
                        KeyValue::new("kind", task.kind().as_str().to_owned()),
                        KeyValue::new("status", "failed"),
                    ],
                );
            }
        }
        Err(fail_err) => {
            let combined = format!("handler error: {error_message}; repo.fail failure: {fail_err}");
            error!(
                task_id = %task.id(),
                error = %fail_err,
                "failed to record task failure"
            );
            emit_task_fail_storage_error(task, ctx.worker_id, &combined, ctx.log_payload);
            if let Some(ref m) = ctx.metrics {
                m.task_duration_seconds.record(
                    elapsed_secs,
                    &[
                        KeyValue::new("queue", ctx.queue_str.clone()),
                        KeyValue::new("kind", task.kind().as_str().to_owned()),
                        KeyValue::new("status", "storage_error"),
                    ],
                );
            }
        }
    }
}

/// Emit the `task_failed_retry`, `task_failed_terminal`, or
/// `task_fail_unexpected_status` lifecycle log depending on the result
/// status from `repo.fail()`.
fn emit_task_failed(
    task: &TaskRecord,
    worker_id: WorkerId,
    record: &TaskRecord,
    error_message: &str,
    log_payload: bool,
) {
    // Use ISO 8601 format for next_scheduled_at.
    // chrono's Display impl uses `YYYY-MM-DD HH:MM:SS UTC` (space
    // separator, UTC suffix), which is not ISO 8601. docs/guidelines/
    // structured-logging.md promises ISO 8601 UTC, and log aggregators
    // expecting an ISO string (Elasticsearch, Loki, CloudWatch) silently
    // drop the field when they see the space-separated form.
    let next_scheduled_at_iso = record.scheduled_at().to_rfc3339();
    match record.status() {
        TaskStatus::Pending => {
            // `max_attempts` sourced
            // from `task.max_attempts` for correlation parity with the
            // sibling `emit_task_fail_storage_error` / `emit_task_fail_panic`
            // events. `max_attempts` is immutable per row so the two
            // sources agree in practice, but using one consistent
            // source removes an operator-filter ambiguity.
            if log_payload {
                warn!(
                    event = "task_failed_retry",
                    task_id = %task.id(),
                    queue = %task.queue(),
                    worker_id = %worker_id,
                    kind = %task.kind(),
                    attempt = %task.attempts(),
                    max_attempts = %task.max_attempts(),
                    next_scheduled_at = %next_scheduled_at_iso,
                    error = %error_message,
                    payload = ?task.payload(),
                    "task failed, will retry"
                );
            } else {
                warn!(
                    event = "task_failed_retry",
                    task_id = %task.id(),
                    queue = %task.queue(),
                    worker_id = %worker_id,
                    kind = %task.kind(),
                    attempt = %task.attempts(),
                    max_attempts = %task.max_attempts(),
                    next_scheduled_at = %next_scheduled_at_iso,
                    error = %error_message,
                    "task failed, will retry"
                );
            }
        }
        TaskStatus::Failed => {
            if log_payload {
                error!(
                    event = "task_failed_terminal",
                    task_id = %task.id(),
                    queue = %task.queue(),
                    worker_id = %worker_id,
                    kind = %task.kind(),
                    attempt = %task.attempts(),
                    max_attempts = %task.max_attempts(),
                    error = %error_message,
                    payload = ?task.payload(),
                    "task failed permanently"
                );
            } else {
                error!(
                    event = "task_failed_terminal",
                    task_id = %task.id(),
                    queue = %task.queue(),
                    worker_id = %worker_id,
                    kind = %task.kind(),
                    attempt = %task.attempts(),
                    max_attempts = %task.max_attempts(),
                    error = %error_message,
                    "task failed permanently"
                );
            }
        }
        other => {
            // Suspended will never be a result of repo.fail() — fail() only transitions
            // running → pending (retry) or running → failed (exhausted).
            // Defense-in-depth branch now
            // carries the same correlation fields as its siblings
            // (queue, worker_id, kind, attempt, max_attempts) so
            // operators debugging a fired unexpected-status event have
            // the same dimensions to filter on.
            error!(
                event = "task_fail_unexpected_status",
                task_id = %task.id(),
                queue = %task.queue(),
                worker_id = %worker_id,
                kind = %task.kind(),
                attempt = %task.attempts(),
                max_attempts = %task.max_attempts(),
                status = ?other,
                error = %error_message,
                "repo.fail returned unexpected status"
            );
        }
    }
}

/// Emit `task_fail_storage_error` for an infrastructure-level failure
/// (missing handler, `repo.complete()` Err, `repo.fail()` Err).
///
/// These sites previously logged via plain
/// `error!(...)` with no canonical lifecycle event, leaving
/// `task_claimed` unpaired in FR19-compliant correlation. This emitter
/// keeps the schema of the other dispatch-side events
/// (`queue`, `worker_id`, `kind`, `attempt`, `error`) so correlation
/// filters work uniformly. Payload inclusion is gated on `log_payload`.
fn emit_task_fail_storage_error(
    task: &TaskRecord,
    worker_id: WorkerId,
    error_message: &str,
    log_payload: bool,
) {
    if log_payload {
        error!(
            event = "task_fail_storage_error",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            max_attempts = %task.max_attempts(),
            error = %error_message,
            payload = ?task.payload(),
            "task dispatch aborted by infrastructure error"
        );
    } else {
        error!(
            event = "task_fail_storage_error",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            max_attempts = %task.max_attempts(),
            error = %error_message,
            "task dispatch aborted by infrastructure error"
        );
    }
}

/// Emit `task_fail_panic` when the handler future panics.
///
/// Handler panics previously resulted in a
/// `task_claimed` event with no terminal pairing — `JoinSet` swallowed
/// the panic silently. The spawned-handler wrapper in `dispatch_task`
/// now detects panics via `JoinError::is_panic` and routes here.
fn emit_task_fail_panic(
    task: &TaskRecord,
    worker_id: WorkerId,
    panic_message: &str,
    log_payload: bool,
) {
    if log_payload {
        error!(
            event = "task_fail_panic",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            max_attempts = %task.max_attempts(),
            error = %panic_message,
            payload = ?task.payload(),
            "handler panicked during execution"
        );
    } else {
        error!(
            event = "task_fail_panic",
            task_id = %task.id(),
            queue = %task.queue(),
            worker_id = %worker_id,
            kind = %task.kind(),
            attempt = %task.attempts(),
            max_attempts = %task.max_attempts(),
            error = %panic_message,
            "handler panicked during execution"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::task_repository::MockTaskRepository;
    use iron_defer_domain::{TaskId, TaskKind, TaskStatus};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Test handler implementations
    // -----------------------------------------------------------------------

    /// A `TaskHandler` that returns a configurable result.
    struct MockHandler {
        kind_str: &'static str,
        result_fn: Box<dyn Fn() -> Result<(), TaskError> + Send + Sync>,
    }

    impl crate::registry::TaskHandler for MockHandler {
        fn kind(&self) -> &'static str {
            self.kind_str
        }

        fn execute<'a>(
            &'a self,
            _payload: &'a serde_json::Value,
            _ctx: &'a TaskContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
            let result = (self.result_fn)();
            Box::pin(async move { result })
        }
    }

    /// A `TaskHandler` that tracks concurrent executions via an `AtomicU32`
    /// and sleeps briefly to simulate work.
    struct ConcurrencyTracker {
        kind_str: &'static str,
        active: Arc<AtomicU32>,
        peak: Arc<AtomicU32>,
    }

    impl crate::registry::TaskHandler for ConcurrencyTracker {
        fn kind(&self) -> &'static str {
            self.kind_str
        }

        fn execute<'a>(
            &'a self,
            _payload: &'a serde_json::Value,
            _ctx: &'a TaskContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
            let active = self.active.clone();
            let peak = self.peak.clone();
            Box::pin(async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_queue() -> QueueName {
        QueueName::try_from("test-queue").expect("valid queue")
    }

    fn synthetic_record(kind: &str) -> TaskRecord {
        let now = chrono::Utc::now();
        TaskRecord::builder()
            .id(TaskId::new())
            .queue(sample_queue())
            .kind(TaskKind::try_from(kind).expect("test kind must be non-empty"))
            .payload(std::sync::Arc::new(serde_json::json!({})))
            .status(TaskStatus::Running)
            .priority(iron_defer_domain::Priority::new(0))
            .attempts(iron_defer_domain::AttemptCount::new(1).unwrap())
            .max_attempts(iron_defer_domain::MaxAttempts::new(3).unwrap())
            .scheduled_at(now)
            .claimed_by(WorkerId::new())
            .claimed_until(now + chrono::Duration::seconds(300))
            .created_at(now)
            .updated_at(now)
            .build()
    }

    fn fast_config() -> WorkerConfig {
        WorkerConfig {
            concurrency: 4,
            poll_interval: Duration::from_millis(10),
            lease_duration: Duration::from_mins(5),
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_mins(30),
            log_payload: false,
            sweeper_interval: Duration::from_mins(1),
            max_claim_backoff: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            idempotency_key_retention: Duration::from_secs(24 * 60 * 60),
            suspend_timeout: Duration::from_secs(24 * 60 * 60),
            region: None,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn worker_claims_and_completes_task() {
        let task = synthetic_record("echo");
        let task_id = task.id();
        let completed_task = task.clone().with_status(TaskStatus::Completed);

        let mut mock_repo = MockTaskRepository::new();

        // First call returns a task, subsequent calls return None
        let task_clone = task.clone();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if call_count_claim.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Some(task_clone.clone()))
            } else {
                Ok(None)
            }
        });

        let completed_clone = completed_task.clone();
        mock_repo
            .expect_complete()
            .withf(move |id| *id == task_id)
            .once()
            .returning(move |_| Ok(completed_clone.clone()));

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler {
            kind_str: "echo",
            result_fn: Box::new(|| Ok(())),
        }));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        // Cancel after enough time for one claim cycle
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn worker_fails_task_on_handler_error() {
        let task = synthetic_record("fail_me");
        let task_id = task.id();
        let failed_task = task.clone().with_status(TaskStatus::Failed);

        let mut mock_repo = MockTaskRepository::new();

        let task_clone = task.clone();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if call_count_claim.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Some(task_clone.clone()))
            } else {
                Ok(None)
            }
        });

        let failed_clone = failed_task.clone();
        mock_repo
            .expect_fail()
            .withf(move |id, _, _, _| *id == task_id)
            .once()
            .returning(move |_, _, _, _| Ok(failed_clone.clone()));

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler {
            kind_str: "fail_me",
            result_fn: Box::new(|| {
                Err(TaskError::ExecutionFailed {
                    kind: iron_defer_domain::ExecutionErrorKind::HandlerFailed {
                        source: "handler error".into(),
                    },
                })
            }),
        }));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn worker_respects_concurrency_limit() {
        let mut mock_repo = MockTaskRepository::new();

        // Return 5 tasks total, then None
        let task_count = Arc::new(AtomicU32::new(0));
        let task_count_claim = task_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if task_count_claim.fetch_add(1, Ordering::SeqCst) < 5 {
                Ok(Some(synthetic_record("tracked")))
            } else {
                Ok(None)
            }
        });

        mock_repo
            .expect_complete()
            .returning(|_| Ok(synthetic_record("tracked")));

        let active = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(ConcurrencyTracker {
            kind_str: "tracked",
            active: active.clone(),
            peak: peak.clone(),
        }));

        let mut config = fast_config();
        config.concurrency = 2;

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(config)
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency {} exceeded limit 2",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn worker_stops_on_cancellation() {
        let mut mock_repo = MockTaskRepository::new();
        mock_repo
            .expect_claim_next()
            .returning(|_, _, _, _| Ok(None));

        let registry = TaskRegistry::new();

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token_clone.cancel();
        });

        let start = tokio::time::Instant::now();
        let result = service.run().await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(
            elapsed < Duration::from_secs(2),
            "worker should stop promptly, took {elapsed:?}",
        );
    }

    #[tokio::test]
    async fn worker_continues_after_claim_error() {
        let task = synthetic_record("echo");
        let task_id = task.id();
        let completed_task = task.clone().with_status(TaskStatus::Completed);

        let mut mock_repo = MockTaskRepository::new();

        let task_clone = task.clone();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            let n = call_count_claim.fetch_add(1, Ordering::SeqCst);
            match n.cmp(&3) {
                std::cmp::Ordering::Less => Err(TaskError::Storage {
                    source: Box::<dyn std::error::Error + Send + Sync>::from(
                        "synthetic transient db failure",
                    ),
                }),
                std::cmp::Ordering::Equal => Ok(Some(task_clone.clone())),
                std::cmp::Ordering::Greater => Ok(None),
            }
        });

        let completed_clone = completed_task.clone();
        mock_repo
            .expect_complete()
            .withf(move |id| *id == task_id)
            .once()
            .returning(move |_| Ok(completed_clone.clone()));

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler {
            kind_str: "echo",
            result_fn: Box::new(|| Ok(())),
        }));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let mut config = fast_config();
        config.max_claim_backoff = Duration::from_millis(50);

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(config)
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok(), "poll loop must not exit on claim errors");
        assert!(
            call_count.load(Ordering::SeqCst) >= 4,
            "expected >= 4 claim attempts, got {}",
            call_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn worker_saturation_classifier_invoked_on_claim_error() {
        use std::sync::atomic::AtomicBool;

        let classifier_called = Arc::new(AtomicBool::new(false));
        let classifier_called_clone = classifier_called.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_claim_next().returning(|_, _, _, _| {
            Err(TaskError::Storage {
                source: Box::<dyn std::error::Error + Send + Sync>::from("sim pool timeout"),
            })
        });

        let registry = TaskRegistry::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let mut config = fast_config();
        config.max_claim_backoff = Duration::from_millis(50);

        let classifier: SaturationClassifier = Arc::new(move |_err| {
            classifier_called_clone.store(true, Ordering::SeqCst);
            true
        });
        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(config)
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .is_saturation(classifier)
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
        assert!(
            classifier_called.load(Ordering::SeqCst),
            "saturation classifier was never invoked"
        );
    }

    #[tokio::test]
    async fn worker_resets_backoff_on_successful_claim() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        let timestamps = Arc::new(std::sync::Mutex::new(Vec::<std::time::Instant>::new()));
        let timestamps_inner = timestamps.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            let n = call_count_claim.fetch_add(1, Ordering::SeqCst);
            timestamps_inner
                .lock()
                .unwrap()
                .push(std::time::Instant::now());
            if n < 2 {
                Err(TaskError::Storage {
                    source: Box::<dyn std::error::Error + Send + Sync>::from("transient"),
                })
            } else {
                Ok(None)
            }
        });

        let registry = TaskRegistry::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let mut config = fast_config();
        config.max_claim_backoff = Duration::from_millis(100);

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(config)
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
        let count = call_count.load(Ordering::SeqCst);
        assert!(
            count >= 4,
            "expected >= 4 claim attempts to verify backoff reset, got {count}"
        );

        let ts = timestamps.lock().unwrap();
        if ts.len() >= 4 {
            let gap_after_reset = ts[3].duration_since(ts[2]);
            assert!(
                gap_after_reset < Duration::from_millis(200),
                "gap after successful claim should be near poll_interval (10ms), was {gap_after_reset:?}"
            );
        }
    }

    #[tokio::test]
    async fn worker_cancellation_races_stuck_claim() {
        use iron_defer_domain::{
            CancelResult, ListTasksFilter, ListTasksResult, QueueStatistics, WorkerStatus,
        };

        struct StuckClaimRepo;

        #[async_trait::async_trait]
        impl TaskRepository for StuckClaimRepo {
            async fn save(&self, _: &TaskRecord) -> Result<TaskRecord, TaskError> {
                unimplemented!()
            }
            async fn save_idempotent(
                &self,
                _: &TaskRecord,
            ) -> Result<(TaskRecord, bool), TaskError> {
                unimplemented!()
            }
            async fn cleanup_expired_idempotency_keys(&self) -> Result<u64, TaskError> {
                unimplemented!()
            }
            async fn find_by_id(&self, _: TaskId) -> Result<Option<TaskRecord>, TaskError> {
                unimplemented!()
            }
            async fn list_by_queue(&self, _: &QueueName) -> Result<Vec<TaskRecord>, TaskError> {
                unimplemented!()
            }
            async fn claim_next(
                &self,
                _: &QueueName,
                _: WorkerId,
                _: Duration,
                _: Option<&str>,
            ) -> Result<Option<TaskRecord>, TaskError> {
                tokio::time::sleep(Duration::from_mins(1)).await;
                Ok(None)
            }
            async fn complete(&self, _: TaskId) -> Result<TaskRecord, TaskError> {
                unimplemented!()
            }
            async fn fail(
                &self,
                _: TaskId,
                _: &str,
                _: f64,
                _: f64,
            ) -> Result<TaskRecord, TaskError> {
                unimplemented!()
            }
            async fn cancel(&self, _: TaskId) -> Result<CancelResult, TaskError> {
                unimplemented!()
            }
            async fn recover_zombie_tasks(
                &self,
            ) -> Result<
                Vec<(
                    TaskId,
                    QueueName,
                    iron_defer_domain::TaskKind,
                    Option<String>,
                    crate::ports::RecoveryOutcome,
                )>,
                TaskError,
            > {
                unimplemented!()
            }
            async fn list_tasks(&self, _: &ListTasksFilter) -> Result<ListTasksResult, TaskError> {
                unimplemented!()
            }
            async fn queue_statistics(
                &self,
                _by_region: bool,
            ) -> Result<Vec<QueueStatistics>, TaskError> {
                unimplemented!()
            }
            async fn worker_status(&self) -> Result<Vec<WorkerStatus>, TaskError> {
                unimplemented!()
            }
            async fn release_leases_for_worker(
                &self,
                _: WorkerId,
            ) -> Result<Vec<(TaskId, Option<String>)>, TaskError> {
                unimplemented!()
            }
            async fn release_lease_for_task(&self, _: TaskId) -> Result<Option<String>, TaskError> {
                unimplemented!()
            }
            async fn audit_log(
                &self,
                _: TaskId,
                _: i64,
                _: i64,
            ) -> Result<iron_defer_domain::ListAuditLogResult, TaskError> {
                unimplemented!()
            }
            async fn suspend(&self, _: TaskId) -> Result<TaskRecord, TaskError> {
                unimplemented!()
            }
            async fn signal(
                &self,
                _: TaskId,
                _: Option<serde_json::Value>,
            ) -> Result<TaskRecord, TaskError> {
                unimplemented!()
            }
            async fn expire_suspended_tasks(
                &self,
                _: Duration,
            ) -> Result<Vec<(TaskId, QueueName)>, TaskError> {
                unimplemented!()
            }
        }

        let registry = TaskRegistry::new();
        let token = CancellationToken::new();
        let token_cancel = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(StuckClaimRepo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        let start = tokio::time::Instant::now();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token_cancel.cancel();
        });

        let _join_set = service.run_poll_loop().await.expect("run_poll_loop");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "worker should exit within 1s of cancellation, took {elapsed:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Payload privacy tests (FR38 / FR39)
    // -----------------------------------------------------------------------

    fn build_privacy_fixture(
        payload: serde_json::Value,
        config: WorkerConfig,
        handler_result: fn() -> Result<(), TaskError>,
    ) -> (
        WorkerService,
        CancellationToken,
        iron_defer_domain::TaskId,
        Arc<tokio::sync::Notify>,
    ) {
        let task = synthetic_record("privacy_kind").with_payload(payload);
        let task_id = task.id();
        let completed_task = task.clone().with_status(TaskStatus::Completed);
        let failed_retry_task = task.clone().with_status(TaskStatus::Pending);

        let dispatch_done = Arc::new(tokio::sync::Notify::new());

        let mut mock_repo = MockTaskRepository::new();
        let task_clone = task.clone();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if call_count_claim.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Some(task_clone.clone()))
            } else {
                Ok(None)
            }
        });

        let completed_clone = completed_task.clone();
        let complete_signal = dispatch_done.clone();
        mock_repo.expect_complete().returning(move |_| {
            complete_signal.notify_one();
            Ok(completed_clone.clone())
        });

        let failed_retry_clone = failed_retry_task.clone();
        let fail_signal = dispatch_done.clone();
        mock_repo.expect_fail().returning(move |_, _, _, _| {
            fail_signal.notify_one();
            Ok(failed_retry_clone.clone())
        });

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler {
            kind_str: "privacy_kind",
            result_fn: Box::new(handler_result),
        }));

        let token = CancellationToken::new();
        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(config)
            .queue(sample_queue())
            .token(token.clone())
            .worker_id(WorkerId::new())
            .build();
        (service, token, task_id, dispatch_done)
    }

    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn payload_privacy_task_completed_hides_payload_by_default() {
        let secret = format!("HIDE_{}", iron_defer_domain::TaskId::new());
        let payload = serde_json::json!({"secret": secret.clone()});

        let (service, token, _task_id, dispatch_done) =
            build_privacy_fixture(payload, fast_config(), || Ok(()));

        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), dispatch_done.notified())
                .await
                .expect("task dispatch did not complete within 5s");
            cancel.cancel();
        });

        service.run().await.expect("worker completed successfully");

        assert!(!logs_contain(&secret));
        assert!(!logs_contain("payload="));
        assert!(logs_contain("task_completed"));
    }

    #[tokio::test]
    async fn worker_handles_missing_handler_gracefully() {
        let task = synthetic_record("unknown_kind");
        let task_id = task.id();

        let mut mock_repo = MockTaskRepository::new();

        let task_clone = task.clone();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if call_count_claim.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Some(task_clone.clone()))
            } else {
                Ok(None)
            }
        });

        let failed_task = task.clone().with_status(TaskStatus::Failed);
        let failed_clone = failed_task.clone();
        mock_repo
            .expect_fail()
            .withf(move |id, msg, _, _| *id == task_id && msg.contains("no handler registered"))
            .once()
            .returning(move |_, _, _, _| Ok(failed_clone.clone()));

        let registry = TaskRegistry::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token_clone.cancel();
        });

        let result = service.run().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn claim_cancelled_releases_lease() {
        let task = synthetic_record("echo");
        let task_clone = task.clone();

        let token = CancellationToken::new();
        let token_for_mock = token.clone();

        let mut mock_repo = MockTaskRepository::new();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_claim = call_count.clone();
        mock_repo.expect_claim_next().returning(move |_, _, _, _| {
            if call_count_claim.fetch_add(1, Ordering::SeqCst) == 0 {
                token_for_mock.cancel();
                Ok(Some(task_clone.clone()))
            } else {
                Ok(None)
            }
        });

        let release_called = Arc::new(AtomicU32::new(0));
        let release_called_clone = release_called.clone();
        mock_repo
            .expect_release_lease_for_task()
            .returning(move |_| {
                release_called_clone.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            });

        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler {
            kind_str: "echo",
            result_fn: Box::new(|| Ok(())),
        }));

        let service = WorkerService::builder()
            .repo(Arc::new(mock_repo) as Arc<dyn TaskRepository>)
            .registry(Arc::new(registry))
            .config(fast_config())
            .queue(sample_queue())
            .token(token)
            .worker_id(WorkerId::new())
            .build();

        let result = service.run().await;
        assert!(result.is_ok());
        assert!(release_called.load(Ordering::SeqCst) >= 1);
    }
}
