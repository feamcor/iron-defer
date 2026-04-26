//! Sweeper service — recovers zombie tasks with expired leases.
//!
//! Architecture references:
//! - §D3.1: Separate `tokio::spawn`'d task with its own interval. Independent
//!   of the worker pool — NOT embedded in the claim loop.
//! - §D6.1: Holds a child `CancellationToken` cloned from the root token.
//!   On cancellation, completes current cycle and exits.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::metrics::Metrics;
use iron_defer_domain::{QueueName, TaskError, TaskId};
use opentelemetry::KeyValue;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};

use crate::ports::{RecoveryOutcome, TaskRepository};
use crate::services::worker::SaturationClassifier;

/// Background sweeper that recovers zombie tasks (running tasks whose lease
/// has expired).
///
/// Runs as an independent `tokio::spawn`'d task. Retryable tasks are reset
/// to `Pending`; exhausted tasks transition to `Failed`.
pub struct SweeperService {
    repo: Arc<dyn TaskRepository>,
    interval: Duration,
    idempotency_key_retention: Duration,
    suspend_timeout: Duration,
    token: CancellationToken,
    is_saturation: SaturationClassifier,
    metrics: Option<Metrics>,
}

impl SweeperService {
    /// Construct a sweeper service.
    ///
    /// The service does not start processing until [`run`](Self::run) is called.
    #[must_use]
    pub fn new(
        repo: Arc<dyn TaskRepository>,
        interval: Duration,
        idempotency_key_retention: Duration,
        token: CancellationToken,
    ) -> Self {
        Self {
            repo,
            interval,
            idempotency_key_retention,
            suspend_timeout: Duration::from_secs(24 * 60 * 60),
            token,
            is_saturation: Arc::new(|_| false),
            metrics: None,
        }
    }

    #[must_use]
    pub fn with_suspend_timeout(mut self, timeout: Duration) -> Self {
        self.suspend_timeout = timeout;
        self
    }

    /// Install `OTel` metric instrument handles for emission at
    /// recovery sites.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Install a pool-saturation classifier for the sweep-loop error branch.
    ///
    /// When the supplied closure returns `true` for a `recover_zombie_tasks`
    /// error, the sweeper emits a `warn!` tagged `event = "pool_saturated"`
    /// instead of the default `error!` line. This mirrors the worker's
    /// NFR-R6 classification so a Postgres outage does not produce a flood
    /// of `error!` logs from the sweeper while the worker correctly warns.
    #[must_use]
    pub fn with_saturation_classifier(mut self, classifier: SaturationClassifier) -> Self {
        self.is_saturation = classifier;
        self
    }

    fn log_recovered_tasks(
        results: &[(
            TaskId,
            QueueName,
            iron_defer_domain::TaskKind,
            Option<String>,
            RecoveryOutcome,
        )],
    ) {
        for (id, queue, kind, trace_id, outcome) in results {
            match outcome {
                RecoveryOutcome::Recovered => {
                    info!(
                        event = "task_recovered",
                        task_id = %id,
                        queue = %queue,
                        kind = %kind,
                        "zombie task recovered"
                    );
                    // Emit transition event for zombie recovery
                    crate::emit_otel_state_transition(
                        trace_id.as_deref(),
                        *id,
                        "running",
                        "pending",
                        queue.as_str(),
                        kind.as_str(),
                        None, // sweeper recovery - no specific worker
                        0,    // attempts unknown
                    );
                }
                RecoveryOutcome::Failed => {
                    warn!(
                        event = "task_failed",
                        task_id = %id,
                        queue = %queue,
                        kind = %kind,
                        "zombie task failed (max attempts exhausted)"
                    );
                    // Emit transition event for zombie exhaustion
                    crate::emit_otel_state_transition(
                        trace_id.as_deref(),
                        *id,
                        "running",
                        "failed",
                        queue.as_str(),
                        kind.as_str(),
                        None,
                        0,
                    );
                }
            }
        }

        // Aggregate summary for batch-level monitoring
        info!(recovered = results.len(), "sweeper recovered zombie tasks");
    }

    fn emit_per_queue_metrics(
        results: &[(
            TaskId,
            QueueName,
            iron_defer_domain::TaskKind,
            Option<String>,
            RecoveryOutcome,
        )],
        metrics: Option<&Metrics>,
    ) {
        if let Some(m) = metrics {
            // Group by queue/kind for counters
            let mut recovery_counts: HashMap<&str, u64> = HashMap::new();
            let mut failure_counts: HashMap<(&str, &str), u64> = HashMap::new();

            for (_, queue, kind, _, outcome) in results {
                *recovery_counts.entry(queue.as_str()).or_default() += 1;
                if let RecoveryOutcome::Failed = outcome {
                    *failure_counts
                        .entry((queue.as_str(), kind.as_str()))
                        .or_default() += 1;
                }
            }

            for (queue, count) in &recovery_counts {
                m.zombie_recoveries_total
                    .add(*count, &[KeyValue::new("queue", queue.to_string())]);
            }

            for ((queue, kind), count) in &failure_counts {
                m.task_failures_total.add(
                    *count,
                    &[
                        KeyValue::new("queue", queue.to_string()),
                        KeyValue::new("kind", kind.to_string()),
                    ],
                );
            }
        }
    }

    /// Run the sweep loop until the cancellation token fires.
    ///
    /// On each tick the sweeper calls `recover_zombie_tasks` to reset
    /// retryable tasks to `Pending` and fail exhausted tasks. Errors from
    /// the repository are logged but do NOT stop the sweeper — it continues
    /// on the next tick.
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` when the cancellation token fires and the loop exits
    /// cleanly. This method does not propagate repository errors.
    #[instrument(
        skip(self),
        fields(
            interval_secs = self.interval.as_secs(),
            idempotency_key_retention_secs = self.idempotency_key_retention.as_secs()
        ),
        err
    )]
    pub async fn run(&self) -> Result<(), TaskError> {
        let mut tick = interval(self.interval);

        info!("sweeper started");

        loop {
            tokio::select! {
                () = self.token.cancelled() => {
                    info!("sweeper received cancellation, stopping");
                    break;
                }
                _ = tick.tick() => {
                    match self.repo.recover_zombie_tasks().await {
                        Ok(results) => {
                            if !results.is_empty() {
                                Self::log_recovered_tasks(&results);
                                Self::emit_per_queue_metrics(&results, self.metrics.as_ref());
                            }
                        }
                        Err(e) => {
                            if (self.is_saturation)(&e) {
                                warn!(
                                    event = "pool_saturated",
                                    error = %e,
                                    "postgres connection pool saturated — sweeper cycle deferred"
                                );
                            } else {
                                error!(error = %e, "sweeper failed to recover zombie tasks");
                            }
                        }
                    }

                    match self.repo.cleanup_expired_idempotency_keys().await {
                        Ok(cleaned) => {
                            if cleaned > 0 {
                                info!(
                                    event = "idempotency_keys_cleaned",
                                    count = cleaned,
                                    "sweeper cleaned expired idempotency keys"
                                );
                                if let Some(ref m) = self.metrics {
                                    m.idempotency_keys_cleaned_total.add(cleaned, &[]);
                                }
                            }
                        }
                        Err(e) => {
                            if (self.is_saturation)(&e) {
                                warn!(
                                    event = "pool_saturated",
                                    error = %e,
                                    "postgres connection pool saturated — idempotency key cleanup deferred"
                                );
                            } else {
                                error!(error = %e, "sweeper failed to clean expired idempotency keys");
                            }
                        }
                    }

                    match self.repo.expire_suspended_tasks(self.suspend_timeout).await {
                        Ok(expired) => {
                            for (id, queue) in &expired {
                                warn!(
                                    event = "suspend_timeout_expired",
                                    task_id = %id,
                                    queue = %queue,
                                    "task auto-failed: suspended too long"
                                );
                                crate::emit_otel_state_transition(
                                    None,
                                    *id,
                                    "suspended",
                                    "failed",
                                    queue.as_str(),
                                    "unknown",
                                    None,
                                    0,
                                );
                                if let Some(ref m) = self.metrics {
                                    m.suspend_timeout_total.add(1, &[KeyValue::new("queue", queue.to_string())]);
                                }
                            }
                        }
                        Err(e) => {
                            if (self.is_saturation)(&e) {
                                warn!(
                                    event = "pool_saturated",
                                    error = %e,
                                    "postgres connection pool saturated — suspend watchdog deferred"
                                );
                            } else {
                                error!(error = %e, "sweeper failed to expire suspended tasks");
                            }
                        }
                    }
                }
            }
        }

        info!("sweeper stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::task_repository::MockTaskRepository;
    use iron_defer_domain::{QueueName, TaskId, TaskKind};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_queue() -> QueueName {
        QueueName::try_from("test-queue").expect("valid queue name")
    }

    fn test_kind() -> TaskKind {
        TaskKind::try_from("test-kind").expect("valid kind")
    }

    #[tokio::test]
    async fn sweeper_calls_recover_on_interval() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_recover_zombie_tasks().returning(move || {
            call_count_inner.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        });
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            Duration::from_millis(10),
            Duration::from_secs(3600),
            token,
        );

        // Let it run for enough time to fire at least 2 ticks
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token_clone.cancel();
        });

        let result = sweeper.run().await;
        assert!(result.is_ok());
        assert!(
            call_count.load(Ordering::SeqCst) >= 2,
            "expected at least 2 recover calls, got {}",
            call_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn sweeper_stops_on_cancellation() {
        let mut mock_repo = MockTaskRepository::new();
        mock_repo
            .expect_recover_zombie_tasks()
            .returning(|| Ok(vec![]));
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            Duration::from_mins(1), // long interval — should not matter
            Duration::from_secs(3600),
            token,
        );

        // Cancel immediately
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            token_clone.cancel();
        });

        let start = tokio::time::Instant::now();
        let result = sweeper.run().await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(
            elapsed < Duration::from_secs(1),
            "sweeper should stop promptly, took {elapsed:?}",
        );
    }

    #[tokio::test]
    async fn sweeper_logs_recovery_count() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_recover_zombie_tasks().returning(move || {
            call_count_inner.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                (
                    TaskId::new(),
                    test_queue(),
                    test_kind(),
                    None,
                    RecoveryOutcome::Recovered,
                ),
                (
                    TaskId::new(),
                    test_queue(),
                    test_kind(),
                    None,
                    RecoveryOutcome::Recovered,
                ),
            ])
        });
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            Duration::from_millis(10),
            Duration::from_secs(3600),
            token,
        );

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token_clone.cancel();
        });

        let result = sweeper.run().await;
        assert!(result.is_ok());
        assert!(
            call_count.load(Ordering::SeqCst) >= 1,
            "recover_zombie_tasks should have been called at least once",
        );
    }

    #[tokio::test]
    async fn sweeper_continues_on_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_recover_zombie_tasks().returning(move || {
            let count = call_count_inner.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                // First call: error
                Err(TaskError::Storage {
                    source: "simulated db error".into(),
                })
            } else {
                Ok(vec![(
                    TaskId::new(),
                    test_queue(),
                    test_kind(),
                    None,
                    RecoveryOutcome::Recovered,
                )])
            }
        });
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            Duration::from_millis(10),
            Duration::from_secs(3600),
            token,
        );

        // Let it run long enough for multiple ticks
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token_clone.cancel();
        });

        let result = sweeper.run().await;
        assert!(result.is_ok());
        // Should have been called more than once despite the first error
        assert!(
            call_count.load(Ordering::SeqCst) >= 2,
            "sweeper should continue after error, got {} calls",
            call_count.load(Ordering::SeqCst)
        );
    }

    /// Per-`TaskId` `task_recovered` emission. For
    /// every id returned by `recover_zombie_tasks()`, the sweeper must
    /// emit one `info!` record tagged `event = "task_recovered"`
    /// alongside the aggregate summary line.
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn sweeper_recovered_event_emitted_per_task_id() {
        let id_a = TaskId::new();
        let id_b = TaskId::new();
        let id_c = TaskId::new();
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();

        let recovered = Arc::new(tokio::sync::Notify::new());
        let recovered_signal = recovered.clone();

        let queue = test_queue();
        let kind = test_kind();
        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_recover_zombie_tasks().returning(move || {
            if call_count_inner.fetch_add(1, Ordering::SeqCst) == 0 {
                recovered_signal.notify_one();
                Ok(vec![
                    (
                        id_a,
                        test_queue(),
                        test_kind(),
                        None,
                        RecoveryOutcome::Recovered,
                    ),
                    (
                        id_b,
                        test_queue(),
                        test_kind(),
                        None,
                        RecoveryOutcome::Recovered,
                    ),
                    (
                        id_c,
                        test_queue(),
                        test_kind(),
                        None,
                        RecoveryOutcome::Recovered,
                    ),
                ])
            } else {
                Ok(vec![])
            }
        });
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let token = CancellationToken::new();
        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            Duration::from_millis(10),
            Duration::from_secs(3600),
            token.clone(),
        );

        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), recovered.notified())
                .await
                .expect("sweeper did not call recover_zombie_tasks within 5s");
            cancel.cancel();
        });

        sweeper.run().await.expect("sweeper run");

        assert!(
            logs_contain("task_recovered"),
            "sweeper did not emit task_recovered event"
        );
        for id in [id_a, id_b, id_c] {
            let id_str = id.to_string();
            assert!(
                logs_contain(&id_str),
                "task_recovered did not carry task_id `{id_str}`"
            );
        }
        assert!(
            logs_contain(queue.as_str()),
            "task_recovered did not carry queue name"
        );
        assert!(
            logs_contain(kind.as_str()),
            "task_recovered did not carry kind"
        );
    }

    /// P1-INT-006 — sweeper interval is configurable and respected: the
    /// sweeper calls `recover_zombie_tasks` at the configured cadence, not
    /// faster. Uses `start_paused = true` for deterministic time control.
    #[tokio::test(start_paused = true)]
    async fn sweeper_interval_configurable_and_respected() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();

        let mut mock_repo = MockTaskRepository::new();
        mock_repo.expect_recover_zombie_tasks().returning(move || {
            call_count_inner.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        });
        mock_repo
            .expect_cleanup_expired_idempotency_keys()
            .returning(|| Ok(0));
        mock_repo
            .expect_expire_suspended_tasks()
            .returning(|_| Ok(vec![]));

        let sweep_interval = Duration::from_secs(10);
        let token = CancellationToken::new();
        let token_cancel = token.clone();

        let sweeper = SweeperService::new(
            Arc::new(mock_repo),
            sweep_interval,
            Duration::from_secs(3600),
            token,
        );

        tokio::spawn(async move { sweeper.run().await });
        tokio::task::yield_now().await;

        // First tick fires immediately.
        let after_first = call_count.load(Ordering::SeqCst);
        assert!(
            after_first >= 1,
            "expected at least 1 call after first tick, got {after_first}"
        );

        // Advance less than one full interval — should NOT trigger another tick.
        tokio::time::advance(
            sweep_interval
                .checked_sub(Duration::from_millis(100))
                .unwrap(),
        )
        .await;
        tokio::task::yield_now().await;
        let mid_interval = call_count.load(Ordering::SeqCst);

        // Advance past the interval boundary.
        tokio::time::advance(Duration::from_millis(200)).await;
        tokio::task::yield_now().await;
        let after_second = call_count.load(Ordering::SeqCst);

        assert!(
            after_second > mid_interval,
            "expected a new recover call after advancing past sweep_interval \
             (mid={mid_interval}, after={after_second})"
        );

        // Advance another full interval — should see exactly one more call.
        let before_third = call_count.load(Ordering::SeqCst);
        tokio::time::advance(sweep_interval).await;
        tokio::task::yield_now().await;
        let after_third = call_count.load(Ordering::SeqCst);

        assert_eq!(
            after_third - before_third,
            1,
            "expected exactly 1 call per interval, got {}",
            after_third - before_third
        );

        token_cancel.cancel();
    }
}
