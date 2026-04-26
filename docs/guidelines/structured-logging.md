# Structured Logging

iron-defer emits one JSON log record per task lifecycle transition (FR19). This document is the operator runbook for the log format, the emission sites, and the privacy controls that gate payload visibility.

- **Format:** newline-delimited JSON on stdout.
- **Formatter:** `tracing_subscriber::fmt::layer().json()` — spans are flattened onto the root event so `task_id`, `queue`, and `worker_id` appear at the top level of every record.
- **Filter:** `RUST_LOG` env var, falling back to `info` (`iron_defer_infrastructure::init_tracing`).
- **Subscriber install site:** `crates/api/src/main.rs` — the standalone binary. The embedded library (`IronDefer::builder`) never installs a global subscriber (Architecture line 776).

---

## Field glossary

| Field               | Type          | Description                                                                                         |
| ------------------- | ------------- | --------------------------------------------------------------------------------------------------- |
| `event`             | string        | Canonical lifecycle event name (see catalogue below).                                               |
| `task_id`           | UUID string   | Task identifier, stable for the task's entire lifetime.                                             |
| `queue`             | string        | Queue name the task belongs to.                                                                     |
| `worker_id`         | UUID string   | Worker process identifier. Stable for the life of the worker.                                       |
| `kind`              | string        | Registered task kind (the `Task::KIND` constant).                                                   |
| `attempt`           | integer       | Attempt number for this execution cycle. Starts at `1` for the first run; `0` in `task_enqueued`. Matches FR19's `attempt_number`. |
| `max_attempts`      | integer       | Retry ceiling for this task.                                                                        |
| `duration_ms`       | integer       | End-to-end handler dispatch time in milliseconds (`task_completed` only).                           |
| `scheduled_at`      | ISO 8601 UTC  | When the task became eligible to claim (`task_enqueued` only).                                      |
| `next_scheduled_at` | ISO 8601 UTC  | Exponential-backoff retry time (`task_failed_retry` only).                                          |
| `error`             | string        | Display form of the `TaskError` (retry/terminal events).                                            |
| `payload`           | JSON value    | **Opt-in only** (see [Payload privacy](#payload-privacy)). Never present under default config.      |
| `filter`            | string        | Effective `EnvFilter` directive at subscriber init (one-shot `tracing subscriber initialized` log). |
| `version`           | string        | `CARGO_PKG_VERSION` at startup (`iron-defer starting` log).                                         |

---

## Lifecycle event catalogue

| Event                         | Level   | Fired by                                             | When                                              | Extra fields                                                       |
| ----------------------------- | ------- | ---------------------------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------ |
| `task_enqueued`               | `info`  | `IronDefer::enqueue_inner` / `enqueue_raw`           | After `SchedulerService::enqueue*` returns Ok     | `priority`, `max_attempts`, `scheduled_at` (+ `payload` if opt-in) |
| `task_claimed`                | `info`  | `WorkerService::run_poll_loop`                       | After `repo.claim_next()` returns `Ok(Some(_))`   | `attempt`                                                          |
| `task_completed`              | `info`  | `dispatch_task`                                      | After `repo.complete()` returns Ok                | `attempt`, `duration_ms`                                           |
| `task_failed_retry`           | `warn`  | `dispatch_task`                                      | `repo.fail()` returned `Pending` status           | `attempt`, `max_attempts`, `next_scheduled_at`, `error`            |
| `task_failed_terminal`        | `error` | `dispatch_task`                                      | `repo.fail()` returned `Failed` status            | `attempt`, `max_attempts`, `error`                                 |
| `task_fail_unexpected_status` | `error` | `dispatch_task`                                      | `repo.fail()` returned a status we don't expect   | `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `status`, `error` |
| `task_fail_storage_error`     | `error` | `dispatch_task` / `handle_task_failure`              | Missing handler registration, `repo.complete()` Err after handler success, or `repo.fail()` Err after handler Err — any infrastructure failure that aborts a dispatch without producing a `Pending`/`Failed` record | `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `error`           |
| `task_fail_panic`             | `error` | `dispatch_task`                                      | Handler future panicked during execution (detected via `tokio::spawn` + `JoinError::is_panic`) | `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `error`           |
| `task_cancelled`              | `info`  | `IronDefer::cancel`                                  | After `SchedulerService::cancel` returns `Cancelled` | `task_id`, `queue`, `kind`                                         |
| `task_recovered`              | `info`  | `SweeperService::run`                                | One record per `TaskId` returned by the sweeper   | `task_id`                                                          |
| `pool_saturated`              | `warn`  | `WorkerService::run_poll_loop` / `SweeperService::run` | `claim_next` / `recover_zombie_tasks` failed with a classifier-matched pool error | `error`                                                            |

The `info!("sweeper recovered zombie tasks", recovered = N)` aggregate summary line is retained alongside the per-task records — useful for dashboards that count batches rather than individual tasks.

---

## Payload privacy

> **Default: `WorkerConfig::log_payload = false`.**

Payloads are never logged by default (FR38 / NFR-S2). Opting in via `WorkerConfig { log_payload: true, ..Default::default() }` appends a `payload = <task.payload>` field to `task_enqueued`, `task_claimed`, `task_completed`, `task_failed_retry`, and `task_failed_terminal` records.

**Security consequence.** Enabling `log_payload` means task payloads — which may contain PII, credentials, or business-sensitive data — flow to every downstream log consumer (shippers, aggregators, cold storage). Operators must verify:

1. The logging pipeline satisfies the same data-classification controls as the database (FR38).
2. No consumer (monitoring dashboards, alert backends, external support tooling) captures payload fields without redaction.
3. Retention on the log store aligns with the domain's data-retention policy.

**Audit-trail compatibility.** The FR40 mutual-exclusion rule targets UNLOGGED mode (where the database itself does not retain task rows). Payload logging is explicitly permitted in combination with the audit trail; they are orthogonal controls.

### DB URL redaction

`sqlx::Error::Configuration` error messages are scrubbed at the `PostgresAdapterError::from(sqlx::Error)` boundary. Any `postgres://` / `postgresql://` substring in the error text has its password segment replaced with `***` via `iron_defer_infrastructure::scrub_url`. This protects the `#[instrument(err)]` serialization chain from leaking the DB URL (NFR-S2, Architecture D4.3).

Scope is intentionally narrow:

- Scrubbed: `sqlx::Error::Configuration(...)` messages that embed a libpq URL.
- Not scrubbed: arbitrary `sqlx::Error::Database` payloads.

---

## `RUST_LOG` tuning

iron-defer honors the standard `tracing_subscriber::EnvFilter` directive grammar. Recipes:

```bash
# Default — info across the workspace.
unset RUST_LOG

# Silence worker poll noise while keeping lifecycle events.
RUST_LOG=iron_defer=info,iron_defer_application::services::worker=warn

# Debug a specific queue: dial everything to trace and filter with jq.
RUST_LOG=trace iron-defer 2>&1 | jq 'select(.queue == "invoices")'

# Follow one task end-to-end.
RUST_LOG=info iron-defer 2>&1 | jq 'select(.task_id == "<uuid>")'

# Surface only terminal failures for alert integration.
RUST_LOG=error iron-defer
```

The effective filter is logged once at startup (`filter` field on the `tracing subscriber initialized` record) so operators can confirm their directive took effect.

---

## Test-time capture

Two patterns are available; both rely on `tracing-test` 0.2.

**Unit tests inside a workspace crate** — the default crate filter works:

```rust
#[tokio::test(flavor = "multi_thread")]
#[tracing_test::traced_test]
async fn my_test() {
    // ... exercise code that emits tracing events ...
    assert!(logs_contain("task_completed"));
}
```

**Integration tests (`crates/*/tests/`)** — compile into a separate crate, so the default per-crate filter drops `iron_defer_*` events. Enable the `no-env-filter` feature on `tracing-test`:

```toml
# crates/api/Cargo.toml
[dev-dependencies]
tracing-test = { workspace = true, features = ["no-env-filter"] }
```

**Span propagation across `tokio::spawn`.** The worker pool already instruments spawned dispatch futures with `.in_current_span()` so events emitted inside `dispatch_task` inherit the `run_poll_loop` span. New worker-layer code that spawns its own tasks should follow the same pattern, otherwise `logs_contain` in tests will not see those events.

Reference harnesses:

- `crates/application/src/services/worker.rs::tests::payload_privacy_*` — unit-test pattern.
- `crates/api/tests/db_outage_integration_test.rs` — integration-test pattern with `no-env-filter`.
- `crates/infrastructure/tests/tracing_privacy_test.rs` — pool-construction secret-leak guard.

---

## Cross-references

- Security OWASP guidance: [security.md §A09](./security.md#a09-security-logging-and-monitoring-failures)
- `#[instrument]` / payload-field discipline: [rust-idioms.md — Payload-Privacy Discipline](./rust-idioms.md#payload-privacy-discipline-fr38)
- Pool-saturation classification (`event = "pool_saturated"`): [postgres-reconnection.md](./postgres-reconnection.md)
- Quality gates: [quality-gates.md](./quality-gates.md)
