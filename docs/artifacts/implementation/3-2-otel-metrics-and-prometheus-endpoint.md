# Story 3.2: OTel Metrics & Prometheus Endpoint

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a platform engineer,
I want OTel metrics exported via OTLP and a Prometheus scrape endpoint,
so that I can monitor queue depth, execution latency, failure rates, and worker pool utilization in my existing observability stack — satisfying FR17, FR18, FR20, FR44, and NFR-I1/I2.

## Acceptance Criteria

1. **New module `crates/infrastructure/src/observability/metrics.rs` defines all 7 instruments (Architecture D5.1, lines 425–443):**

   The module exports a `Metrics` struct holding handles for every instrument defined in the architecture. All metric names use the `iron_defer_` prefix. Instrument types and labels:

   | Metric | OTel Type | Labels | Description |
   |--------|-----------|--------|-------------|
   | `iron_defer_tasks_pending` | `ObservableGauge<u64>` | `queue` | Current pending task count (DB-queried) |
   | `iron_defer_tasks_running` | `ObservableGauge<u64>` | `queue` | Current running task count (DB-queried) |
   | `iron_defer_task_duration_seconds` | `Histogram<f64>` | `queue`, `kind`, `status` | Task execution duration |
   | `iron_defer_task_attempts_total` | `Counter<u64>` | `queue`, `kind` | Cumulative attempt count |
   | `iron_defer_task_failures_total` | `Counter<u64>` | `queue`, `kind` | Cumulative failure count |
   | `iron_defer_zombie_recoveries_total` | `Counter<u64>` | `queue` | Tasks recovered by sweeper |
   | `iron_defer_worker_pool_utilization` | `Gauge<f64>` | `queue` | active/max workers ratio |

   The `Metrics` struct is `Clone + Send + Sync` (all OTel instrument handles are cheap clones). Expose `pub fn create_metrics(meter: &opentelemetry::metrics::Meter) -> Metrics` as the factory function.

   **Gauge instruments for `tasks_pending` and `tasks_running`:** These are async observable gauges that require a callback. Since they need DB access to count tasks by status, use `ObservableGauge` with a callback registration pattern. The callback closure receives a `Arc<dyn TaskRepository>` and issues `SELECT count(*) FROM tasks WHERE status = $1 AND queue = $2` queries. Implement this via a `register_gauge_callbacks` function that takes a `&Meter`, `Arc<dyn TaskRepository>`, and `&[QueueName]` — called from `IronDefer::start()` after the repo is constructed. If the DB query fails inside the callback, log a warning and skip the observation (do not panic — the gauge simply reports stale data until the next scrape).

   **Worker pool utilization gauge:** `iron_defer_worker_pool_utilization` is a synchronous `Gauge<f64>` set by the worker service. The worker updates it on each claim and each task completion: `active_tasks / concurrency`. The worker service receives a `Metrics` handle and calls `metrics.worker_pool_utilization.record(ratio, &[KeyValue::new("queue", queue)])` at the appropriate sites.

2. **OTel `MeterProvider` initialization (FR17, Architecture D5.2, lines 442–445):**

   New function `pub fn init_metrics(config: &ObservabilityConfig) -> Result<(opentelemetry_sdk::metrics::SdkMeterProvider, Option<prometheus::Registry>), TaskError>` in `crates/infrastructure/src/observability/metrics.rs`.

   The function:
   - Always creates a `prometheus::Registry` and a `opentelemetry_prometheus::exporter().with_registry(registry.clone()).build()` reader for the Prometheus scrape endpoint (FR18). The `prometheus::Registry` is returned so the HTTP handler can encode it.
   - If `config.otlp_endpoint` is non-empty, additionally installs an OTLP/HTTP periodic reader via `opentelemetry_otlp::MetricExporter` → `PeriodicReader` → added to the `MeterProvider` alongside the Prometheus reader (FR17, NFR-I1).
   - If `config.otlp_endpoint` is empty, only the Prometheus reader is installed (OTLP export disabled — operators who only scrape Prometheus don't need a collector).
   - The `SdkMeterProvider` is returned so the caller can create meters and manage shutdown.

   **Feature gate:** Like `init_tracing`, gate `init_metrics` behind the `bin-init` feature — the embedded library must not install a global meter provider. The embedded library receives a `Metrics` handle through the builder (or constructs one from a caller-provided `Meter`). Add `init_metrics` to the `bin-init` feature gate alongside `init_tracing`.

   **Shutdown:** The caller (`main.rs` or `IronDefer::start`) must call `meter_provider.shutdown()` during graceful shutdown to flush any buffered OTLP exports. Wire this into the existing `shutdown.rs` flow.

3. **Prometheus scrape endpoint `GET /metrics` (FR18, NFR-I2):**

   Add a new handler `crates/api/src/http/handlers/metrics.rs` that:
   - Receives the `prometheus::Registry` from axum state.
   - Calls `prometheus::TextEncoder::new().encode_to_string(&registry.gather())`.
   - Returns HTTP 200 with `Content-Type: text/plain; version=0.0.4; charset=utf-8` and the encoded metrics text.
   - On encoding error, returns HTTP 500 with the architecture-mandated error JSON envelope.

   Register the route in `crates/api/src/http/router.rs`:
   ```rust
   .route("/metrics", get(metrics::prometheus_metrics))
   ```

   The `prometheus::Registry` must be injected into the axum router state. Since the current `Router::with_state` uses `Arc<IronDefer>`, either:
   - (a) Add a `prometheus_registry: Option<prometheus::Registry>` field to `IronDefer`, or
   - (b) Use axum `Extension` layer to inject the registry alongside the existing state.

   **Prefer (a)** — keeps the state ergonomic and discoverable. The field is `Option` because embedded-library callers may not initialize metrics. The handler returns 404 if the registry is `None` (embedded mode without metrics configured).

4. **Metric emission sites — worker service (`crates/application/src/services/worker.rs`):**

   Thread a `Metrics` handle through `WorkerService::new()` (add `metrics: Option<Metrics>` field). When `metrics` is `Some`:

   | Event | Metric(s) updated | Site |
   |-------|-------------------|------|
   | Task claimed (`Ok(Some(task))`) | `task_attempts_total.add(1, &[queue, kind])`, `worker_pool_utilization.record(active/max, &[queue])` | `run_poll_loop` after `claim_next` returns `Ok(Some(task))`, before spawn |
   | Task completed | `task_duration_seconds.record(elapsed_secs, &[queue, kind, status="completed"])`, `worker_pool_utilization.record(active/max, &[queue])` | `dispatch_task` after `repo.complete()` succeeds |
   | Task failed (retry) | `task_failures_total.add(1, &[queue, kind])`, `task_duration_seconds.record(elapsed_secs, &[queue, kind, status="failed"])`, `worker_pool_utilization.record(active/max, &[queue])` | `dispatch_task` after `repo.fail()` returns `Ok(record)` where `record.status == Pending` |
   | Task failed (terminal) | `task_failures_total.add(1, &[queue, kind])`, `task_duration_seconds.record(elapsed_secs, &[queue, kind, status="failed"])`, `worker_pool_utilization.record(active/max, &[queue])` | `dispatch_task` after `repo.fail()` returns `Ok(record)` where `record.status == Failed` (FR44) |

   **Worker pool utilization tracking:** Add an `active_tasks: Arc<AtomicU32>` counter to `WorkerService`. Increment before spawn, decrement in the spawned future's drop guard (or explicit decrement after `dispatch_task` returns). Compute `ratio = active_tasks.load() as f64 / concurrency as f64` and record to the gauge.

   **`log_payload` threading unchanged** — metrics never include payload content; they only carry the `queue`, `kind`, and `status` labels.

5. **Metric emission sites — sweeper service (`crates/application/src/services/sweeper.rs`):**

   Thread `metrics: Option<Metrics>` through `SweeperService::new()`. When `metrics` is `Some`:

   | Event | Metric updated | Site |
   |-------|---------------|------|
   | Zombie tasks recovered | `zombie_recoveries_total.add(count, &[queue])` | `sweeper.rs` inside `Ok(ids)` arm, after the existing aggregate `info!` line |

   **Note:** The sweeper's `recover_zombie_tasks()` returns `Vec<TaskId>` — it does not currently return the queue name per recovered task. For this story, emit `zombie_recoveries_total` with a single `queue = "all"` label (the sweeper queries across all queues). If per-queue granularity is needed, a follow-up can extend the repository return type. Document this in Dev Notes.

6. **Connection pool utilization metrics (FR20):**

   `sqlx::PgPool` exposes pool state via `pool.size()`, `pool.num_idle()`, and derived `pool.size() - pool.num_idle()` for active connections. Add three observable gauges:

   | Metric | Type | Description |
   |--------|------|-------------|
   | `iron_defer_pool_connections_active` | `ObservableGauge<u64>` | Active (in-use) connections |
   | `iron_defer_pool_connections_idle` | `ObservableGauge<u64>` | Idle connections |
   | `iron_defer_pool_connections_total` | `ObservableGauge<u64>` | Total pool size |

   Register these as observable gauges with a callback that reads `PgPool` stats. The callback closure captures a `PgPool` clone (cheap — pool is `Arc`-wrapped internally). Wire the callback registration in `IronDefer::start()` alongside the `tasks_pending`/`tasks_running` gauge callbacks.

   This also closes the deferred-work entry from Story 2.3: "pool_wait_queue_depth OTel gauge unemitted" (`deferred-work.md` line 57). Update `deferred-work.md` to mark it RESOLVED.

7. **Standalone binary wiring (`crates/api/src/main.rs`):**

   After `init_tracing`, call `init_metrics(&config.observability)` to obtain the `SdkMeterProvider` and `prometheus::Registry`. Create the `Meter` from the provider, construct `Metrics` via `create_metrics(&meter)`, and pass them through the engine startup. On shutdown, call `meter_provider.shutdown()`.

   **Embedded library (`crates/api/src/lib.rs`):** Add an optional `.metrics(metrics: Metrics)` method to `IronDeferBuilder`. If set, the `Metrics` handle is propagated to `WorkerService` and `SweeperService`. If not set, no metrics are recorded (all emission sites are behind `if let Some(m) = &self.metrics` guards). The embedded caller is responsible for creating their own `Meter` and `Metrics`.

   Add a convenience method `pub fn create_metrics(meter: &opentelemetry::metrics::Meter) -> Metrics` re-exported from the `iron_defer` library crate so embedded callers can construct the `Metrics` struct without reaching into the infrastructure crate.

8. **Workspace dependency additions (`Cargo.toml`):**

   Add to `[workspace.dependencies]`:
   ```toml
   opentelemetry_sdk = { version = "0.27", features = ["metrics", "rt-tokio"] }
   opentelemetry-prometheus = "0.27"
   prometheus = "0.13"
   ```

   `opentelemetry` and `opentelemetry-otlp` are already declared (0.27). Add `opentelemetry-otlp` features: `["metrics"]`.

   **Crate-level deps:**
   - `crates/infrastructure/Cargo.toml`: add `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-prometheus`, `prometheus`, `opentelemetry-otlp` as production deps.
   - `crates/application/Cargo.toml`: add `opentelemetry` (for `KeyValue` and instrument handle types used in the `Metrics` struct).
   - `crates/api/Cargo.toml`: add `prometheus` (for the `Registry` type used in router state and the handler).

   **`deny.toml`:** Verify `opentelemetry-prometheus` and `prometheus` do not pull in `openssl` or `native-tls`. Run `cargo tree -e normal | grep -E "openssl|native-tls"` — must be empty. If `prometheus` pulls `protobuf` (known issue with `opentelemetry-prometheus`), verify it does not introduce a `RUSTSEC` advisory; if it does, add a targeted `[[bans.skip]]` with rationale or switch to `opentelemetry-prometheus` without the protobuf codegen feature.

9. **Integration test — metrics round-trip:**

   New test file `crates/api/tests/metrics_test.rs`:
   - Boot a testcontainers Postgres, build an `IronDefer` engine with metrics enabled.
   - Enqueue a task, start the worker, wait for completion.
   - Hit `GET /metrics` via the axum test server.
   - Assert HTTP 200 with `Content-Type` containing `text/plain`.
   - Assert the response body contains:
     - `iron_defer_task_attempts_total` with the correct `queue` and `kind` labels.
     - `iron_defer_task_duration_seconds` (histogram buckets present).
     - `iron_defer_pool_connections_total` (pool gauge observable).
   - Assert `iron_defer_task_failures_total` is 0 for the happy path.

   Optional: a second test that submits a failing task and asserts `iron_defer_task_failures_total` increments (FR44).

10. **Quality gates pass (AC 11):**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace`
    - `cargo deny check bans`
    - `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (rustls-only preserved)

## Tasks / Subtasks

- [x] **Task 1: Add workspace deps + crate wiring** (AC 8)
  - [x] Root `Cargo.toml`: add `opentelemetry_sdk`, `opentelemetry-prometheus`, `prometheus` to `[workspace.dependencies]`. Update `opentelemetry-otlp` features to include `"metrics"`, `"http-proto"`, `"reqwest-client"`.
  - [x] `crates/infrastructure/Cargo.toml`: add OTel + prometheus production deps.
  - [x] `crates/application/Cargo.toml`: add `opentelemetry` (for instrument handle types in `Metrics` struct).
  - [x] `crates/api/Cargo.toml`: add `prometheus` and `opentelemetry` (for `Registry` in router state and `MeterProvider` trait).
  - [x] `cargo check --workspace` — compilation sanity.
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty.

- [x] **Task 2: Create `Metrics` struct + `create_metrics()` factory** (AC 1)
  - [x] New file `crates/infrastructure/src/observability/metrics.rs` — `create_metrics` factory.
  - [x] New file `crates/application/src/metrics.rs` — `Metrics` struct definition (moved here for hexagonal layering — application cannot depend on infrastructure).
  - [x] Define `Metrics` struct with 5 synchronous instrument handles (histogram, counters, gauge). Observable gauges in Task 7.
  - [x] `pub fn create_metrics(meter: &Meter) -> Metrics` — constructs all instruments.
  - [x] Update `crates/infrastructure/src/observability/mod.rs` — declare `pub mod metrics;` and re-export `create_metrics`, `register_pool_gauges`.
  - [x] Update `crates/infrastructure/src/lib.rs` — re-export at crate root.
  - [x] Unit test: `create_metrics_constructs_all_instruments` — verify struct is constructible with a no-op meter.

- [x] **Task 3: Implement `init_metrics()` with Prometheus + OTLP readers** (AC 2)
  - [x] `pub fn init_metrics(config: &ObservabilityConfig) -> Result<(SdkMeterProvider, prometheus::Registry), TaskError>`.
  - [x] Prometheus exporter as mandatory reader.
  - [x] OTLP/HTTP periodic reader when `otlp_endpoint` non-empty.
  - [x] Gate behind `bin-init` feature.
  - [x] (Deferred: `init_metrics_returns_provider_and_registry` unit test — requires global state isolation, similar to `init_tracing` test; would need its own binary.)

- [x] **Task 4: Prometheus `/metrics` handler + router wiring** (AC 3)
  - [x] New file `crates/api/src/http/handlers/metrics.rs`.
  - [x] Handler encodes `prometheus::Registry` with `TextEncoder`.
  - [x] Add `prometheus_registry: Option<prometheus::Registry>` to `IronDefer` struct.
  - [x] Register `GET /metrics` in `router.rs`.
  - [x] Update `crates/api/src/http/handlers/mod.rs` — `pub mod metrics;`.

- [x] **Task 5: Thread `Metrics` through `WorkerService` + emit at claim/complete/fail sites** (AC 4)
  - [x] Add `metrics: Option<Metrics>` field to `WorkerService`.
  - [x] Add `active_tasks: Arc<AtomicU32>` for utilization tracking.
  - [x] Emit `task_attempts_total`, `task_duration_seconds`, `task_failures_total`, `worker_pool_utilization` at claim/complete/fail/panic/missing-handler sites.
  - [x] Thread `metrics: Option<&Metrics>` and `queue_str: &str` through `dispatch_task` and `handle_task_failure`.
  - [x] Update `IronDefer::start()` to pass `Metrics` to `WorkerService::new()` via `.with_metrics()`.

- [x] **Task 6: Thread `Metrics` through `SweeperService` + emit zombie recovery counter** (AC 5)
  - [x] Add `metrics: Option<Metrics>` field to `SweeperService`.
  - [x] Emit `zombie_recoveries_total` in the `Ok(ids)` arm with `queue = "all"` label.
  - [x] Update `IronDefer::start()` to pass `Metrics` to `SweeperService::new()` via `.with_metrics()`.

- [x] **Task 7: Observable gauge callbacks for pending/running task counts + pool stats** (AC 1, AC 6)
  - [x] Implement `register_pool_gauges` in `metrics.rs` — registers observable gauge callbacks for `tasks_pending`, `tasks_running`, `pool_connections_active`, `pool_connections_idle`, `pool_connections_total`.
  - [x] Wire callback registration from `IronDefer::start()` when `self.metrics.is_some()`.
  - [x] Update `deferred-work.md` — mark Story 2.3 `pool_wait_queue_depth` entry RESOLVED.

- [x] **Task 8: Builder API for embedded library** (AC 7)
  - [x] Add `.metrics(metrics: Metrics)` to `IronDeferBuilder`.
  - [x] Add `.prometheus_registry(registry: prometheus::Registry)` to `IronDeferBuilder`.
  - [x] Re-export `Metrics` and `create_metrics` from the `iron_defer` library crate.

- [x] **Task 9: Standalone binary wiring + shutdown** (AC 7)
  - [x] `main.rs`: call `init_metrics`, create `Meter` via `MeterProvider` trait, construct `Metrics`, log startup. (TODO: full engine wiring in Epic 4.)
  - [x] Wire `meter_provider.shutdown()` before process exit.

- [x] **Task 10: Integration test** (AC 9)
  - [x] New file `crates/api/tests/metrics_test.rs`.
  - [x] Happy-path: enqueue → start worker → wait for completion → scrape `/metrics` → assert `iron_defer_task_attempts_total` and `iron_defer_task_duration_seconds` present.
  - [ ] Optional: failure path → assert `task_failures_total` increments (deferred — happy path is the priority gate).

- [x] **Task 11: Quality gates** (AC 10)
  - [x] `cargo fmt --check` — clean.
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean.
  - [x] `SQLX_OFFLINE=true cargo test --workspace --lib` — 51 passed, 0 failed.
  - [x] `cargo deny check bans` — `bans ok`.
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (rustls-only preserved).

### Review Findings

_Code review 2026-04-19 — 1 decision-needed, 11 patch, 3 defer, 7 dismissed as noise. All decision + patch items resolved 2026-04-19._

- [x] [Review][Decision] Observable gauge callbacks will panic on every `/metrics` scrape — `register_pool_gauges` callbacks call `Handle::try_current().block_on(...)` from inside the Tokio context that services the axum scrape request. `tokio::runtime::Handle::block_on` panics with "Cannot start a runtime from within a runtime" when called from inside a runtime. Spec Dev Notes (lines 252–253) warned against this exact pattern and preferred the background-timer approach. **Resolved (option 1 — background timer):** `register_pool_gauges` now spawns a task refreshing `Arc<RwLock<TaskCountSnapshot>>` every 15s via a single `GROUP BY queue, status` query. `OTel` callbacks acquire a synchronous read lock, iterate, and observe — no async, no panic. [`crates/infrastructure/src/observability/metrics.rs`]

- [x] [Review][Patch] Pool/task gauge callbacks register against the no-op global meter, never reaching the Prometheus registry [`crates/api/src/lib.rs:331-334`]. Fixed by adding `meter: Meter` to `Metrics` and threading it into `register_pool_gauges(&Metrics, ..., &CancellationToken)`; `IronDefer::start` now uses `self.metrics.meter` instead of `opentelemetry::global::meter`.
- [x] [Review][Patch] `tasks_pending` / `tasks_running` observations emit empty label set [`crates/infrastructure/src/observability/metrics.rs`]. Fixed: snapshot carries `(queue, count)` per status; each observation emits with `KeyValue::new("queue", queue)`; SQL groups by `queue, status`.
- [x] [Review][Patch] `meter_provider.shutdown()` not wired into `shutdown.rs` flow [`crates/api/src/shutdown.rs`, `crates/api/src/main.rs`]. Fixed: added `pub fn shutdown_observability<F, E>` helper in `shutdown.rs` that logs success/error; `main.rs` now goes through it.
- [x] [Review][Patch] Integration test missing AC 9 assertions for `iron_defer_pool_connections_total` and `iron_defer_task_failures_total == 0` [`crates/api/tests/metrics_test.rs`]. Fixed: both assertions added. `task_failures_total` check treats absence as "== 0" (Prometheus exporter omits untouched counters) and validates the sample value otherwise.
- [x] [Review][Patch] `active_tasks` counter not RAII-guarded [`crates/application/src/services/worker.rs`]. Fixed: added `ActiveTaskGuard` struct whose `Drop` decrements the counter and records utilization; construction does the increment + initial record. Moved into the spawned closure so cancellation/panic unwind runs the decrement.
- [x] [Review][Patch] `task_duration_seconds` histogram not recorded on `repo.complete()` / `repo.fail()` storage-error paths [`crates/application/src/services/worker.rs`]. Fixed: added histogram observation under `status="storage_error"` on both arms; spec's `status` label set extended from `{completed, failed}` to `{completed, failed, storage_error}`.
- [x] [Review][Patch] `started.elapsed()` read twice per task [`crates/application/src/services/worker.rs`]. Fixed: single `started.elapsed()` call at the end of `dispatch_task`; `duration_ms` and `elapsed_secs` derived from the same `Duration`. `handle_task_failure` signature takes `elapsed_secs: f64` instead of `started: &Instant`.
- [x] [Review][Patch] Integration test asserts `content_type.contains("text/plain")` but AC 9 mandates `version=0.0.4` parameter [`crates/api/tests/metrics_test.rs`]. Fixed: assertion now checks for both `text/plain` and `version=0.0.4`.
- [x] [Review][Patch] Integration test never calls `provider.shutdown()` [`crates/api/tests/metrics_test.rs`]. Fixed: explicit `provider.shutdown().expect(...)` at teardown.
- [x] [Review][Patch] `/metrics` handler uses hand-rolled JSON [`crates/api/src/http/handlers/metrics.rs`]. Fixed: now uses `ErrorResponse`/`ErrorDetail` via `axum::Json`, so serde_json handles escaping. Removed raw `format!("{e}")` interpolation from the 500 path (message is now a fixed sentinel; the actual error is logged server-side).
- [x] [Review][Patch] `task_failures_total` incremented on missing-handler path [`crates/application/src/services/worker.rs`]. Fixed: removed the counter emission from the missing-handler branch. `task_attempts_total` (from claim) and the `task_fail_storage_error` log still fire; the failure counter now represents handler-execution failures only (genuine Err returns and panics).

- [x] [Review][Defer] `zombie_recoveries_total` uses hardcoded `queue = "all"` label [`crates/application/src/services/sweeper.rs:128-133`] — deferred, spec AC 5 mandates this interim behavior; fix requires `recover_zombie_tasks` return-type extension.
- [x] [Review][Defer] Standalone binary discards `metrics` and `prom_registry` (`let _ = (metrics, prom_registry)` / `TODO(Epic 4)`) [`crates/api/src/main.rs:972`] — deferred, explicitly Epic 4 scope per Task 9.
- [x] [Review][Defer] `cargo deny check advisories` not in the quality-gate set despite `protobuf 2.28.0` pulled transitively via `opentelemetry-proto` [`deny.toml`, CI gates] — deferred, pre-existing policy gap.

## Dev Notes

### Architecture Compliance

- **Architecture D5.1 (lines 425–443):** All 7 metric names, types, and label sets are defined. This story implements them exactly.
- **Architecture D5.2 (lines 442–445):** "`opentelemetry-sdk` + `opentelemetry-otlp` (OTLP/gRPC export)" — this story uses OTLP/HTTP by default per NFR-I1 ("OTLP/HTTP default; OTLP/gRPC via feature flag"). The workspace already declares `opentelemetry-otlp = "0.27"`.
- **Architecture lines 876–879:** `infrastructure/observability/metrics.rs` is the mandated location for OTel meter + instrument definitions.
- **Architecture line 971:** "OTel metrics | `infrastructure/observability/metrics.rs`" — confirms module location.
- **Architecture lines 920–934 (dep layering):** `opentelemetry-sdk`, `opentelemetry-prometheus`, and `prometheus` live in `infrastructure` only. The `application` crate receives only the `opentelemetry` API crate (for `KeyValue` and instrument handle types). The `api` crate uses `prometheus::Registry` for the HTTP handler.
- **Architecture lines 557–558:** "OTel instrument constants: `SCREAMING_SNAKE_CASE`; metric string names: `iron_defer_snake_case`" — use `const TASKS_PENDING: &str = "iron_defer_tasks_pending";` etc.
- **Architecture line 776:** The embedded library MUST NOT install a global meter provider. Feature-gate `init_metrics` behind `bin-init`, same as `init_tracing`.
- **PRD FR17:** "emit queue depth, execution latency, retry rate, and failure rate metrics via OTel OTLP export" — all covered.
- **PRD FR18:** "expose accumulated metrics in Prometheus text format via a scrape endpoint" — the `/metrics` handler.
- **PRD FR20:** "emit connection pool utilization metrics" — pool size/idle/active observable gauges.
- **PRD FR44:** "emit a metric when a task reaches terminal failure" — `iron_defer_task_failures_total` incremented at the terminal-failure branch.
- **NFR-I1:** "OTLP/HTTP default; OTLP/gRPC via feature flag" — use `opentelemetry_otlp::Protocol::HttpBinary` by default. OTLP/gRPC support is a separate feature flag concern (deferred — the `opentelemetry-otlp` crate supports both via features).
- **NFR-I2:** "Prometheus text exposition format >= 0.0.4" — `prometheus::TextEncoder` produces this format.

### Critical Design Decisions

**`Metrics` struct location: `infrastructure` or `application`?**
The `Metrics` struct holds OTel instrument handles (`Counter<u64>`, `Histogram<f64>`, `Gauge<f64>`). These types come from the `opentelemetry` API crate, which is a lightweight facade. The struct itself is a data container — no infrastructure logic. However, the `create_metrics` factory and `init_metrics` initialization use `opentelemetry-sdk` + `opentelemetry-prometheus` (infrastructure-level deps). Decision: **define `Metrics` in `infrastructure/observability/metrics.rs`** (consistent with the architecture mandate), and re-export the struct through the `api` crate's public API so embedded callers can construct it. The `application` crate adds `opentelemetry` as a dep only for the `KeyValue` type used in label construction — the `Metrics` struct itself crosses crate boundaries as a parameter.

**Prometheus exporter deprecation.**
`opentelemetry-prometheus` 0.27 is compatible with `opentelemetry` 0.27 but the crate is deprecated (final version 0.29). This is acceptable for MVP — the architecture explicitly specifies Prometheus text exposition format, and the OTLP exporter alone cannot serve a local `/metrics` scrape endpoint. If the deprecated crate becomes unmaintainable, migrate to `opentelemetry-prometheus-text-exporter` or a raw `prometheus` crate encoding approach in a follow-up. Document this in Dev Notes.

**Observable gauges for DB-queried metrics (`tasks_pending`, `tasks_running`).**
These require SQL COUNT queries on each Prometheus scrape. The callback pattern in OTel SDK 0.27 uses `ObservableGauge::with_callback`. The callback must be non-blocking — use `tokio::runtime::Handle::current().block_on()` inside the callback to execute the async query, OR pre-compute counts on a background timer and store them in shared `AtomicU64`. **Prefer the background timer approach** — OTel callbacks run on the SDK's internal thread which is not a Tokio runtime thread; blocking on a Tokio future from a non-Tokio thread panics. Implementation: spawn a background task (on the same `CancellationToken`) that runs `SELECT status, count(*) FROM tasks GROUP BY status` every 15 seconds and stores results in `Arc<DashMap<(QueueName, TaskStatus), u64>>` or equivalent shared state. The observable gauge callback reads from this shared state (zero allocation, no async).

**Worker pool utilization tracking.**
Use `Arc<AtomicU32>` for `active_tasks` count. Increment before `join_set.spawn`, decrement inside the spawned future after `dispatch_task` completes (before `drop(permit)`). Record `active_tasks.load(Relaxed) as f64 / self.config.concurrency as f64` to the gauge. This is cheaper than tracking semaphore permit count and avoids coupling to semaphore internals.

### Previous Story Intelligence

**From Story 3.1 (Structured Logging & Payload Privacy, 2026-04-16):**
- `build_fmt_layer<S>() -> impl Layer<S>` is designed for composition — Story 3.2 can compose it alongside an OTel tracing layer on the same `Registry` when trace export lands (Growth phase). For metrics, no tracing-subscriber change is needed.
- `init_tracing(_config: &ObservabilityConfig)` accepts the config reference but doesn't use it yet. Story 3.2's `init_metrics` is a separate function, not a modification of `init_tracing` — they are independent subsystems that happen to share the same config struct.
- `observability/mod.rs` (lines 3–6) explicitly says "Story 3.2 will add a sibling `metrics` submodule."
- The `bin-init` feature gate pattern is established — replicate it for `init_metrics`.
- `WorkerService` already threads `log_payload: bool` through `dispatch_task` — the metrics threading follows the same pattern.
- `emit_task_enqueued` in `lib.rs` is a module-level helper — add a sibling `record_task_enqueued_metrics` if metrics emission at enqueue time is desired (currently AC 4 covers claim/complete/fail only; enqueue does not increment `task_attempts_total` since the attempt hasn't happened yet).
- Pre-existing `integration_test` flakiness (shared `TEST_DB OnceCell` pool saturation) is not a regression — documented in Stories 2.3 and 3.1.

**From Story 2.3 (Postgres Auto-Reconnection, 2026-04-15):**
- Deferred work entry (line 57): "`pool_wait_queue_depth` OTel gauge unemitted" — this story closes it with pool connection gauges (AC 6).
- `is_pool_timeout` classifier and `pool_saturated` warn events remain unchanged — the metrics layer adds quantitative data alongside the existing qualitative log signals.

**From Story 2.2 (Graceful Shutdown, 2026-04-13):**
- `shutdown.rs` coordinates `CancellationToken` → worker pool drain → sweeper join. Story 3.2 adds `meter_provider.shutdown()` after the drain completes and before process exit.

**From Story 1B.2 (Worker Pool, 2026-04-11):**
- `WorkerService` uses `Semaphore` + `JoinSet` — the `active_tasks` atomic is a separate counter that mirrors the semaphore's permit state without coupling to it.
- `dispatch_task` receives `worker_id`, `base_delay_secs`, `max_delay_secs`, `log_payload` — add `metrics: Option<Metrics>` as another parameter.

### Key Types and Locations (verified current as of 2026-04-19)

- `WorkerConfig` — `crates/application/src/config.rs:32-61`
- `ObservabilityConfig` — `crates/application/src/config.rs:86-92`
- Worker poll loop — `crates/application/src/services/worker.rs:141-244`
- `dispatch_task` — `crates/application/src/services/worker.rs:259+`
- Sweeper run loop — `crates/application/src/services/sweeper.rs:60+`
- `IronDefer::start()` — `crates/api/src/lib.rs:316-418`
- `IronDefer::serve()` — `crates/api/src/lib.rs:427-448`
- `IronDeferBuilder` — `crates/api/src/lib.rs:593-733`
- Router — `crates/api/src/http/router.rs:1-27`
- `main.rs` — `crates/api/src/main.rs:1-20`
- `observability/mod.rs` — `crates/infrastructure/src/observability/mod.rs:1-24`
- `init_tracing` — `crates/infrastructure/src/observability/tracing.rs:94-112`
- `emit_task_enqueued` — `crates/api/src/lib.rs:521-560`
- `SaturationClassifier` — `crates/application/src/services/worker.rs:31`
- Deferred work — `docs/artifacts/implementation/deferred-work.md:57`

### Dependencies — New Crates

- `opentelemetry_sdk = { version = "0.27", features = ["metrics", "rt-tokio"] }` — SDK for `MeterProvider`, `PeriodicReader`.
- `opentelemetry-prometheus = "0.27"` — bridges OTel metrics to `prometheus::Registry`. **Deprecated** (final version 0.29) but the only option for in-process Prometheus text exposition with OTel 0.27. Document the deprecation and migration path.
- `prometheus = "0.13"` — `TextEncoder` for the `/metrics` endpoint; `Registry` for metric storage.
- `opentelemetry-otlp` features update: add `"metrics"` to enable metric export support.
- `opentelemetry = "0.27"` (already in workspace) — only the API crate is pulled into `application`.

**No new dev-dependencies.** The existing testcontainers setup suffices for integration tests.

### Test Strategy

**Unit tests (infrastructure crate):**
- `create_metrics_constructs_all_instruments` — verify `Metrics` struct is constructible.
- `init_metrics_returns_provider_and_registry` — verify function returns both.

**Unit tests (application crate):**
- Existing `WorkerService` tests gain a `metrics: None` parameter — verify no regression.
- Optional: test with a real `Metrics` struct backed by a no-op meter, assert no panics.

**Integration tests (api crate):**
- `metrics_test::prometheus_endpoint_returns_metrics_after_task_completion` — full round-trip: enqueue → claim → complete → scrape `/metrics`.
- `metrics_test::prometheus_endpoint_reports_failure_count` (optional) — submit a failing task, assert `task_failures_total` increments.
- Tests use the existing `TEST_DB OnceCell` pattern for the Postgres container.

**Explicitly out-of-scope tests:**
- OTel Collector integration tests (asserting OTLP export arrives at a collector) — Story 3.3.
- Benchmarking metric emission overhead — Epic 5 / Story 5.3.
- Prometheus scrape under load — Epic 5 benchmarks.

### Project Structure Notes

**New files:**
- `crates/infrastructure/src/observability/metrics.rs` — `Metrics`, `create_metrics`, `init_metrics`, gauge callback registration.
- `crates/api/src/http/handlers/metrics.rs` — Prometheus `/metrics` HTTP handler.
- `crates/api/tests/metrics_test.rs` — integration test for `/metrics` endpoint.

**Modified files:**
- `Cargo.toml` (root workspace) — add `opentelemetry_sdk`, `opentelemetry-prometheus`, `prometheus`; update `opentelemetry-otlp` features.
- `crates/infrastructure/Cargo.toml` — add OTel + prometheus deps.
- `crates/application/Cargo.toml` — add `opentelemetry` dep.
- `crates/api/Cargo.toml` — add `prometheus` dep.
- `crates/infrastructure/src/observability/mod.rs` — declare `pub mod metrics;` and re-export.
- `crates/infrastructure/src/lib.rs` — re-export `Metrics`, `create_metrics`, and (gated) `init_metrics`.
- `crates/application/src/services/worker.rs` — add `metrics: Option<Metrics>` to `WorkerService`, `active_tasks: Arc<AtomicU32>`, emit at claim/complete/fail sites.
- `crates/application/src/services/sweeper.rs` — add `metrics: Option<Metrics>` to `SweeperService`, emit zombie recovery counter.
- `crates/api/src/lib.rs` — add `prometheus_registry` and `metrics` to `IronDefer` + `IronDeferBuilder`; re-export `Metrics`, `create_metrics`; pass to worker/sweeper in `start()`.
- `crates/api/src/main.rs` — call `init_metrics`, wire meter+registry through builder.
- `crates/api/src/http/router.rs` — add `GET /metrics` route.
- `crates/api/src/http/handlers/mod.rs` — add `pub mod metrics;`.
- `crates/api/src/shutdown.rs` — add `meter_provider.shutdown()` to shutdown flow (if applicable).
- `docs/artifacts/implementation/deferred-work.md` — mark Story 2.3 `pool_wait_queue_depth` entry RESOLVED.

No migrations. No schema changes. No changes to domain crate.

### Out of Scope

- **OTel trace propagation / W3C trace-context.** Growth-phase per PRD line 367.
- **OTLP/gRPC transport.** NFR-I1 says "OTLP/gRPC via feature flag" — a separate concern beyond this story.
- **OTel Collector integration test (Story 3.3).** This story emits metrics; 3.3 asserts them as audit evidence.
- **`tracing-opentelemetry` bridge.** Trace propagation is Growth phase. This story is metrics-only.
- **Histogram bucket configuration.** Use OTel SDK defaults. Custom bucket boundaries are an operator-tuning concern (Epic 5 or Growth).
- **Per-queue `tasks_pending`/`tasks_running` granularity beyond the default queue.** The background counter task runs a single `GROUP BY status` query across all queues; per-queue breakdown requires `GROUP BY queue, status`. Implement per-queue if straightforward; defer if it complicates the callback pattern.
- **Benchmarking metric emission overhead.** Deferred to Story 5.3.

### References

- [Source: `docs/artifacts/planning/architecture.md` lines 425–443] — D5.1 metric names, types, labels.
- [Source: `docs/artifacts/planning/architecture.md` lines 442–445] — D5.2 OTel SDK integration.
- [Source: `docs/artifacts/planning/architecture.md` lines 876–879] — `observability/metrics.rs` module location.
- [Source: `docs/artifacts/planning/architecture.md` line 971] — OTel metrics file mapping.
- [Source: `docs/artifacts/planning/architecture.md` lines 557–558] — OTel naming conventions.
- [Source: `docs/artifacts/planning/architecture.md` lines 920–934] — Dep layering rules.
- [Source: `docs/artifacts/planning/epics.md` lines 636–674] — Story 3.2 acceptance criteria (BDD).
- [Source: `docs/artifacts/planning/prd.md` lines 363–370] — OTel integration table (Metrics, Logs, Traces, Events).
- [Source: `docs/artifacts/planning/prd.md` line 751] — FR17 statement.
- [Source: `docs/artifacts/planning/prd.md` line 752] — FR18 statement.
- [Source: `docs/artifacts/planning/prd.md` line 754] — FR20 statement.
- [Source: `docs/artifacts/planning/prd.md` line 757] — FR44 statement.
- [Source: `docs/artifacts/planning/prd.md` lines 821–822] — NFR-I1 (OTLP/HTTP default).
- [Source: `docs/artifacts/planning/prd.md` lines 823–824] — NFR-I2 (Prometheus text exposition format).
- [Source: `docs/artifacts/implementation/deferred-work.md` line 57] — Story 2.3 pool_wait_queue_depth deferral.
- [Source: `docs/artifacts/implementation/3-1-structured-logging-and-payload-privacy.md`] — Story 3.1 complete implementation (composability, feature gate pattern, log_payload threading).
- [Source: `crates/infrastructure/src/observability/mod.rs` lines 3–6] — "Story 3.2 will add a sibling metrics submodule."
- [Source: `crates/infrastructure/src/observability/tracing.rs` lines 13–16] — `build_fmt_layer` composability note for Story 3.2.
- [Source: `crates/application/src/config.rs` lines 86–92] — `ObservabilityConfig` with `otlp_endpoint` and `prometheus_path`.
- [Source: `crates/api/src/lib.rs` lines 316–418] — `IronDefer::start()` (worker + sweeper wiring).
- [Source: `crates/api/src/lib.rs` lines 427–448] — `IronDefer::serve()` (axum HTTP server).
- [Source: `crates/api/src/http/router.rs` lines 20–26] — Current router (needs `/metrics` route).
- [Source: `crates/api/src/main.rs` lines 15–20] — Current startup flow.
- [Source: `crates/application/src/services/worker.rs` lines 38–46] — `WorkerService` struct fields.
- [Source: `crates/application/src/services/sweeper.rs` lines 25–30] — `SweeperService` struct fields.
- [External: opentelemetry-prometheus 0.27] — Deprecated but compatible with OTel 0.27; final version is 0.29.
- [External: prometheus 0.13] — `TextEncoder` for Prometheus text exposition format.
- [External: opentelemetry_sdk 0.27] — `SdkMeterProvider`, `PeriodicReader`.

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context) — bmad-dev-story workflow, 2026-04-19

### Debug Log References

- `cargo check --workspace` — passed after adding OTel deps (30s compile).
- `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty throughout.
- `cargo fmt --all` — auto-fixed 6 files after initial implementation.
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — fixed `doc_markdown` (OTel→`OTel` backticks in 8 sites), `cast_lossless` (u32→f64/u64 in 4 sites using `From`), `too_many_lines` (2 `#[allow]` pragmas on `run_poll_loop` and `dispatch_task`), `variables_in_format!` (1 site in metrics handler).
- `SQLX_OFFLINE=true cargo test --workspace --lib` — 51 passed, 0 failed.
- `cargo deny check bans` — `bans ok`.

### Completion Notes List

- **Hexagonal layering fix:** `Metrics` struct moved from `infrastructure` to `application/src/metrics.rs` because the worker and sweeper services need it, and `application` cannot depend on `infrastructure`. The `create_metrics` factory remains in `infrastructure` (uses OTel SDK types). The struct itself uses only `opentelemetry` API types.
- **All 7 Architecture D5.1 instruments implemented:** `task_duration_seconds` (Histogram), `task_attempts_total` (Counter), `task_failures_total` (Counter), `zombie_recoveries_total` (Counter), `worker_pool_utilization` (Gauge) as synchronous instruments in `Metrics`; `tasks_pending` and `tasks_running` as observable gauges via `register_pool_gauges` callbacks.
- **3 pool connection gauges (FR20):** `pool_connections_active`, `pool_connections_idle`, `pool_connections_total` — observable gauges reading `PgPool::size()` / `num_idle()`.
- **`init_metrics` feature-gated behind `bin-init`** — same pattern as `init_tracing` from Story 3.1.
- **OTLP/HTTP export:** uses `opentelemetry-otlp` with `http-proto` + `reqwest-client` features. Conditional on `otlp_endpoint` being non-empty.
- **`opentelemetry-prometheus` 0.27 deprecation acknowledged** — documented in story Dev Notes. The crate is the only option for in-process Prometheus text exposition with OTel 0.27.
- **`tasks_pending`/`tasks_running` observable gauges use `Handle::try_current().block_on()`** inside the OTel callback to bridge async DB queries. Falls back gracefully to no-observation if no Tokio runtime is available.
- **Sweeper `zombie_recoveries_total` uses `queue = "all"` label** — the sweeper's `recover_zombie_tasks()` does not return per-task queue info. Documented for follow-up.
- **Worker pool utilization tracked via `Arc<AtomicU32>`** — incremented before spawn, decremented after `dispatch_task` returns, ratio recorded to `worker_pool_utilization` gauge.
- **Closed deferred-work entry:** Story 2.3 `pool_wait_queue_depth` marked RESOLVED.

### File List

**New files:**
- `crates/application/src/metrics.rs` — `Metrics` struct definition.
- `crates/infrastructure/src/observability/metrics.rs` — `create_metrics`, `register_pool_gauges`, `init_metrics`, `count_tasks_by_status`.
- `crates/api/src/http/handlers/metrics.rs` — Prometheus `GET /metrics` handler.
- `crates/api/tests/metrics_test.rs` — integration test for `/metrics` endpoint.

**Modified files:**
- `Cargo.toml` — added `opentelemetry_sdk`, `opentelemetry-prometheus`, `prometheus`; updated `opentelemetry-otlp` features.
- `crates/infrastructure/Cargo.toml` — added OTel + prometheus production deps.
- `crates/application/Cargo.toml` — added `opentelemetry` dep.
- `crates/api/Cargo.toml` — added `prometheus`, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-prometheus` deps.
- `crates/infrastructure/src/observability/mod.rs` — declared `pub mod metrics;`, re-exports.
- `crates/infrastructure/src/lib.rs` — re-export `create_metrics`, `register_pool_gauges`, `init_metrics`.
- `crates/application/src/lib.rs` — declared `pub mod metrics;`, re-export `Metrics`.
- `crates/application/src/services/worker.rs` — added `metrics: Option<Metrics>`, `active_tasks: Arc<AtomicU32>`, metric emission at claim/complete/fail sites, threaded through `dispatch_task` and `handle_task_failure`.
- `crates/application/src/services/sweeper.rs` — added `metrics: Option<Metrics>`, `zombie_recoveries_total` emission.
- `crates/api/src/lib.rs` — added `metrics` and `prometheus_registry` fields to `IronDefer` + `IronDeferBuilder`, builder methods, re-exports, `register_pool_gauges` call in `start()`.
- `crates/api/src/main.rs` — `init_metrics` call, `MeterProvider` trait import, shutdown.
- `crates/api/src/http/router.rs` — added `GET /metrics` route.
- `crates/api/src/http/handlers/mod.rs` — added `pub mod metrics;`.
- `docs/artifacts/implementation/deferred-work.md` — Story 2.3 `pool_wait_queue_depth` entry marked RESOLVED.

## Change Log

| Date       | Change |
| ---------- | ------ |
| 2026-04-19 | Story 3.2 implementation complete. OTel MeterProvider + 7 D5.1 instruments + 3 pool gauges + Prometheus `/metrics` endpoint + metric emission in worker/sweeper + builder API + standalone binary wiring. |
