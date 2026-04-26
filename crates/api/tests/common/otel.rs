//! Per-test `OTel` harness (Story 3.3 AC 1 / C3 resolution).
//!
//! Every integration test in `crates/api/tests/` that needs metric
//! evidence builds a fresh harness via [`build_harness`]. The harness
//! owns:
//!
//! - a private `prometheus::Registry` (never shared across tests — shared
//!   registries leak sample state between tests and corrupt assertions),
//! - an `opentelemetry-prometheus` exporter attached to that registry,
//! - a `SdkMeterProvider` built against the exporter,
//! - the `Meter` used to construct the `iron_defer` instrument handles.
//!
//! Tests drive the engine with `metrics(harness.metrics.clone())` and
//! `prometheus_registry(harness.registry.clone())`, call
//! `harness.provider.shutdown()` on teardown (`OTel` SDK contract — see
//! Story 3.2 review P9), then scrape via [`scrape_samples`] for
//! assertions.
//!
//! The Prometheus registry IS the compliance oracle (Dev Notes: C3
//! blocker resolution). No mock OTLP receiver is built; OTLP egress is
//! feature-gated behind `bin-init` on a separate code path.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use iron_defer::{IronDefer, Metrics, TaskStatus, create_metrics};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use prometheus::{Registry, TextEncoder};
use tokio_util::sync::CancellationToken;

/// Handles returned by [`build_harness`].
///
/// The caller moves `metrics` and `registry` into the engine builder,
/// retains the other two for scrape + shutdown. Dropping without
/// calling `provider.shutdown()` is legal but leaves async export paths
/// un-flushed — AC 3 / AC 5 assertions depend on the flush, so tests
/// exercising histograms and gauges MUST shut down before scraping.
#[allow(dead_code)]
pub struct TestHarness {
    pub provider: SdkMeterProvider,
    pub meter: opentelemetry::metrics::Meter,
    pub metrics: Metrics,
    pub registry: Registry,
}

/// One parsed Prometheus text-exposition sample.
///
/// Counters / gauges expose one `Sample` per label-set. Histograms
/// expose their `_count`, `_sum`, and `_bucket{le=...}` lines as
/// distinct `PromSample` entries (the Prometheus text format treats
/// them as separate metric families — we do not merge them).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PromSample {
    pub metric: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

/// Construct a fresh `OTel` + Prometheus harness.
///
/// Each call allocates a new `prometheus::Registry` so tests remain
/// independent. The `Meter` is named `"iron_defer_test"` — the name is
/// a cosmetic instrumentation-scope label and not asserted on.
#[must_use]
#[allow(dead_code)]
pub fn build_harness() -> TestHarness {
    let registry = Registry::new();
    let prom_exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .expect("prometheus exporter");
    let provider = SdkMeterProvider::builder()
        .with_reader(prom_exporter)
        .build();
    let meter = provider.meter("iron_defer_test");
    let metrics = create_metrics(&meter);
    TestHarness {
        provider,
        meter,
        metrics,
        registry,
    }
}

/// Scrape the Prometheus registry and return every sample line as a
/// [`PromSample`].
///
/// Deliberately hand-rolled (~40 LOC) so the test harness avoids a
/// crate-graph dependency on `prometheus-parse` — same tool-pickiness
/// as Story 3.1's decision to avoid `opentelemetry-appender-tracing`.
#[must_use]
#[allow(dead_code)]
pub fn scrape_samples(registry: &Registry) -> Vec<PromSample> {
    let encoder = TextEncoder::new();
    let mut buf = String::new();
    encoder
        .encode_utf8(&registry.gather(), &mut buf)
        .expect("prometheus text encode");

    let mut out = Vec::new();
    for line in buf.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((head, value_str)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(value) = value_str.parse::<f64>() else {
            continue;
        };

        let (metric, labels) = if let Some(brace_start) = head.find('{') {
            let metric = &head[..brace_start];
            // Strip trailing `}` — Prometheus text format guarantees it
            // when `{` opened the label block.
            let inner = head[brace_start + 1..].trim_end_matches('}');
            (metric.to_owned(), parse_labels(inner))
        } else {
            (head.to_owned(), BTreeMap::new())
        };

        out.push(PromSample {
            metric,
            labels,
            value,
        });
    }
    out
}

/// Parse the `k1="v1",k2="v2"` interior of a Prometheus label block.
///
/// Keeps the parser self-contained; values are already unquoted in the
/// Prometheus text format before this point. Backslash-escapes inside
/// label values are not un-escaped — no iron-defer metric label carries
/// `\"` or `\n`, so the identity mapping is sufficient for the test
/// oracle.
fn parse_labels(inner: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // Split on `,` outside quotes. Label values are always quoted in
    // Prometheus text exposition, so toggling on `"` is sufficient.
    let mut in_quotes = false;
    let mut start = 0;
    let bytes = inner.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                insert_label(&inner[start..i], &mut out);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < inner.len() {
        insert_label(&inner[start..], &mut out);
    }
    out
}

fn insert_label(pair: &str, out: &mut BTreeMap<String, String>) {
    let pair = pair.trim();
    if pair.is_empty() {
        return;
    }
    let Some((k, v)) = pair.split_once('=') else {
        return;
    };
    let v = v.trim().trim_start_matches('"').trim_end_matches('"');
    out.insert(k.trim().to_owned(), v.to_owned());
}

/// Poll `engine.list(&queue)` until every task reaches a terminal state
/// (`Completed` / `Failed` / `Cancelled`) or the budget expires.
///
/// Returns `true` on success, `false` on timeout (with diagnostics
/// printed to stderr showing which tasks are stuck and in what status).
#[allow(dead_code)]
pub async fn await_all_terminal(
    engine: &IronDefer,
    queue: &str,
    attempts_budget: u32,
    interval: Duration,
) -> bool {
    for _ in 0..attempts_budget {
        tokio::time::sleep(interval).await;
        let tasks = engine.list(queue).await.expect("list tasks");
        if !tasks.is_empty()
            && tasks.iter().all(|t| {
                matches!(
                    t.status(),
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
            })
        {
            return true;
        }
    }
    let tasks = engine.list(queue).await.expect("list tasks (diagnostic)");
    let stuck: Vec<_> = tasks
        .iter()
        .filter(|t| {
            !matches!(
                t.status(),
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            )
        })
        .map(|t| format!("{}: {:?}", t.id(), t.status()))
        .collect();
    let total_ms =
        u64::from(attempts_budget) * u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
    eprintln!(
        "await_all_terminal timed out after {attempts_budget} polls (~{total_ms}ms): \
         {}/{} tasks stuck — {}",
        stuck.len(),
        tasks.len(),
        stuck.join(", ")
    );
    false
}

/// Run a closure under a fresh worker token, then cancel + await the
/// join handle. Callers rely on the engine being built with
/// `shutdown_timeout = 1 s` to bound the wait.
#[allow(dead_code)]
pub async fn with_worker<F, Fut>(engine: Arc<IronDefer>, body: F)
where
    F: FnOnce(Arc<IronDefer>, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let token = CancellationToken::new();
    let worker_cancel = token.clone();
    let worker_engine = engine.clone();
    let worker_handle = tokio::spawn(async move {
        worker_engine
            .start(worker_cancel)
            .await
            .expect("engine start");
    });

    body(engine, token.clone()).await;

    token.cancel();
    match tokio::time::timeout(Duration::from_secs(5), worker_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) if join_err.is_panic() => {
            std::panic::resume_unwind(join_err.into_panic());
        }
        Ok(Err(join_err)) => panic!("worker task join error: {join_err}"),
        Err(elapsed) => panic!(
            "worker did not exit within 5 s of cancellation ({elapsed}) — likely a drain-timeout bug"
        ),
    }
}

/// Find the first sample whose metric name + label subset matches.
#[allow(dead_code)]
pub fn find_sample<'a>(
    samples: &'a [PromSample],
    metric: &str,
    label_subset: &[(&str, &str)],
) -> Option<&'a PromSample> {
    samples.iter().find(|s| {
        s.metric == metric
            && label_subset
                .iter()
                .all(|(k, v)| s.labels.get(*k).map(String::as_str) == Some(*v))
    })
}
