//! Story 3.1 Task 5 — api-level payload-privacy verification for the
//! `task_enqueued` lifecycle emission site (`crates/api/src/lib.rs`).
//!
//! The worker-side tests in `crates/application/src/services/worker.rs`
//! cover the five dispatch-side events (claim / complete / `failed_retry` /
//! `failed_terminal`). This file closes the loop at the api layer, which
//! is the sole emission site for `task_enqueued` (AC 3 rationale — see
//! Story 3.1 Dev Notes "Scheduler vs api for `task_enqueued` emission").
//!
//! The `tracing-test` dev-dep for this crate is wired with the
//! `no-env-filter` feature (`crates/api/Cargo.toml`), otherwise the
//! per-crate default filter would drop events emitted by
//! `iron_defer::IronDefer::enqueue_inner` when they are captured from
//! this integration-test binary.

mod common;

use iron_defer::{IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

/// Test task type scoped to this module so we don't collide with other
/// integration-test suites that also define tasks named `EchoTask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrivacyTask {
    /// Field name is deliberately mundane — the assertion checks for the
    /// unique per-run marker the test embeds in this value, not for the
    /// field name itself.
    marker: String,
}

impl Task for PrivacyTask {
    const KIND: &'static str = "observability_privacy_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        // Payload privacy at enqueue time does not depend on execute() — we
        // never start the worker in this test, so this body is unreachable.
        Ok(())
    }
}

/// FR38 / AC 4 verification at the api layer: `IronDefer::enqueue` fires
/// `event = "task_enqueued"` without leaking the payload under default
/// `WorkerConfig { log_payload: false, .. }`.
///
/// The `enqueue_inner` emission site is synchronous w.r.t. the caller
/// (it runs before `enqueue().await` returns), so `tracing_test`'s
/// scoped capture sees the event deterministically without needing to
/// spawn a worker.
#[tokio::test(flavor = "multi_thread")]
#[tracing_test::traced_test]
async fn payload_privacy_task_enqueued_hides_payload_by_default() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable — skipping api-level task_enqueued privacy test");
        return;
    };
    let pool = &pool;

    // Unique secret keeps this test robust against parallel execution —
    // `logs_contain` is scoped to the current `#[traced_test]` future,
    // but using a per-run token is a belt-and-braces defense against any
    // stray subscriber leakage.
    let secret = format!("ENQ_HIDE_{}", uuid::Uuid::new_v4());
    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<PrivacyTask>()
        .queue(&queue)
        .build()
        .await
        .expect("build engine");

    let task = PrivacyTask {
        marker: secret.clone(),
    };
    engine.enqueue(&queue, task).await.expect("enqueue task");

    // Positive control — confirm the event actually fired; without this a
    // silent subscriber breakage would produce a vacuously passing test.
    assert!(
        logs_contain("task_enqueued"),
        "task_enqueued event was not emitted at the api layer"
    );

    // FR38: default `log_payload = false` — neither the literal field name
    // nor the embedded secret value may appear in the captured output.
    assert!(
        !logs_contain(&secret),
        "payload marker `{secret}` leaked through task_enqueued log (default privacy broken)"
    );
    // Story 3.1 second-pass review (P4): tracing-test renders fields as
    // `key=value` (no quotes), so the probe for the payload field name
    // must match `payload=`. The prior `"payload"` (quoted) probe was a
    // literal string that never appears in tracing-test output and would
    // never catch an FR38 regression.
    assert!(
        !logs_contain("payload="),
        "`payload=` field appeared in task_enqueued log (FR38 default broken at api layer)"
    );
}

/// P2-INT-006 — FR39 opt-in verification at the api layer:
/// `IronDefer::enqueue` fires `event = "task_enqueued"` WITH the payload
/// when `WorkerConfig { log_payload: true, .. }` is set.
#[tokio::test(flavor = "multi_thread")]
#[tracing_test::traced_test]
async fn payload_privacy_task_enqueued_shows_payload_when_opted_in() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable — skipping api-level task_enqueued opt-in test");
        return;
    };
    let pool = &pool;

    let secret = format!("ENQ_SHOW_{}", uuid::Uuid::new_v4());
    let queue = common::unique_queue();

    let mut config = WorkerConfig::default();
    config.log_payload = true;

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<PrivacyTask>()
        .queue(&queue)
        .worker_config(config)
        .build()
        .await
        .expect("build engine");

    let task = PrivacyTask {
        marker: secret.clone(),
    };
    engine.enqueue(&queue, task).await.expect("enqueue task");

    assert!(
        logs_contain("task_enqueued"),
        "task_enqueued event was not emitted at the api layer"
    );

    assert!(
        logs_contain("payload="),
        "`payload=` field missing from task_enqueued log (FR39 opt-in broken at api layer)"
    );
    assert!(
        logs_contain(&secret),
        "payload marker `{secret}` not found in task_enqueued log (FR39 opt-in broken)"
    );
}
