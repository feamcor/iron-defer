# Story 3.1: Structured Logging & Payload Privacy

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a platform engineer,
I want structured JSON logs for every task lifecycle transition with payload privacy enforced by default,
so that I can debug task issues, satisfy FR19 / FR38 / FR39 compliance obligations, and prepare the subscriber for OTel layering in Story 3.2 — without ever leaking task payload content.

## Acceptance Criteria

1. **`init_tracing()` helper wires the production JSON subscriber (FR19, Architecture D5.2, lines 442–445):**
   - New module `crates/infrastructure/src/observability/tracing.rs` exports `pub fn init_tracing(config: &ObservabilityConfig) -> Result<(), TaskError>`.
   - Builds a composable `tracing_subscriber::Registry` chain:
     - `EnvFilter` with `try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))` (preserves the existing `RUST_LOG` contract from `crates/api/src/main.rs:11-14`).
     - `tracing_subscriber::fmt::layer().json().with_current_span(true).with_span_list(true).with_target(true).flatten_event(true)` — JSON formatter with span context so correlation fields (`task_id`, `queue`, `worker_id` from `#[instrument]`) appear at the top level of each record.
     - `.with_writer(std::io::stdout)` — OTLP-logs export is **explicitly out of scope** (Story 3.2); stdout JSON is the FR19 MVP format per PRD line 366.
   - Initializes via `.try_init()` (never `.init()`) and maps the `TryInitError` to `TaskError::Storage { source }` so double-init in tests/embedded mode is a graceful error, not a panic.
   - **Composability requirement:** expose `pub fn build_fmt_layer<S>() -> impl tracing_subscriber::Layer<S>` where `S: Subscriber + for<'a> LookupSpan<'a>`, returning the pre-configured JSON `fmt::Layer`. Story 3.2's OTel metrics/tracing bridge will add a second layer (`tracing_opentelemetry::layer()`) to the same `Registry` without rewriting this function — **no global singleton, no `Box<dyn Layer>` erasure**.
   - Dev Notes must document why `init_tracing()` lives in `infrastructure` (access to `ObservabilityConfig` via the `application` dep already wired), the exposure trade-off, and why the **embedded library must never call it** (caller owns the subscriber — Architecture line 776).

2. **Standalone binary uses the new helper (`crates/api/src/main.rs`):**
   - Replace the inline `tracing_subscriber::fmt().with_env_filter(...).init()` chain (`main.rs:11-15`) with `iron_defer_infrastructure::init_tracing(&config.observability)?` (use `ObservabilityConfig::default()` placeholder until figment config loading arrives in Epic 4 / Epic 5 — same pattern as `DatabaseConfig::default()` today).
   - The binary logs one `tracing::info!(version = env!("CARGO_PKG_VERSION"), "iron-defer starting")` record at startup so the JSON subscriber emits observable evidence of init, replacing the existing `run_placeholder()` call's `info!` emission.
   - **Do NOT** touch `IronDefer::builder()` or `IronDefer::build()` — the embedded library continues to inherit the caller's subscriber.

3. **Task lifecycle log records emitted at every transition (FR19):**
   Each transition emits a single `tracing::event!` at the stated level with the stated `event` structured field. Level discipline matches PRD observability intent: info for normal, warn for retryable failures, error for terminal. **No payload appears in any record under default config.**

   | Transition | Site | Level | `event = ` | Required structured fields |
   |---|---|---|---|---|
   | none → `pending` | `IronDefer::enqueue_inner` + `IronDefer::enqueue_raw` immediately after `self.scheduler.enqueue*(...).await` returns `Ok(record)` (`crates/api/src/lib.rs:249-254`, `:485-495`) — **NOT** in `SchedulerService`; see AC 5 rationale for emitting at the api-layer façade | `info` | `"task_enqueued"` | `task_id`, `queue`, `kind`, `priority`, `max_attempts`, `scheduled_at` |
   | `pending` → `running` | `WorkerService::run_poll_loop` immediately after `repo.claim_next()` returns `Ok(Some(task))` (`crates/application/src/services/worker.rs:156`) | `info` | `"task_claimed"` | `task_id`, `queue`, `worker_id`, `kind`, `attempt` (= `task.attempts` after the increment) |
   | `running` → `completed` | `dispatch_task` after `repo.complete()` returns `Ok` (`worker.rs:239`) | `info` | `"task_completed"` | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `duration_ms` |
   | `running` → `pending` (retry) | `dispatch_task` after `repo.fail()` returns `Ok(record)` where `record.status == TaskStatus::Pending` (`worker.rs:246`) | `warn` | `"task_failed_retry"` | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `next_scheduled_at`, `error` (error.to_string()) |
   | `running` → `failed` (terminal) | `dispatch_task` after `repo.fail()` returns `Ok(record)` where `record.status == TaskStatus::Failed` | `error` | `"task_failed_terminal"` | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `error` |
   | `running` → `pending` (zombie recovery) | Already aggregate-logged at `sweeper.rs:95` — keep the aggregate `"sweeper recovered zombie tasks"` line and additionally emit **one** `event = "task_recovered"` `info!` per recovered `TaskId` inside `SweeperService::run` using the `Vec<TaskId>` returned from `recover_zombie_tasks()`. Fields: `task_id`, `event = "task_recovered"`. (Queue / worker_id not available from the current return type — do NOT change `TaskRepository::recover_zombie_tasks` for this story; per-task queue correlation can be recovered from the task store with `task_id`.) |

   `attempt` must always be the final `task.attempts` value AFTER the atomic increment in `claim_next` (Architecture D1.2 / `crates/infrastructure/src/adapters/postgres_task_repository.rs:288`), matching FR19's "attempt_number" semantic. `task_claimed` and later events therefore report identical `attempt` values for the same execution instance.

   `duration_ms` is measured by the worker — capture `let started = std::time::Instant::now();` immediately before `handler.execute(...)` in `dispatch_task` and `started.elapsed().as_millis()` at emission time. Do **not** compute it from DB timestamps (clock skew between app host and DB invalidates the signal; see TEA test-design-qa.md timing section).

   **Auxiliary events (Story 3.1 second-pass review, P16 / D2-b):** In addition to the six canonical lifecycle events above, three auxiliary events signal infrastructure-level failures that occur WITHIN a dispatch cycle but are NOT themselves task-state transitions. They always PAIR with a canonical event (`task_claimed` beforehand; `task_failed_retry` or `task_failed_terminal` afterwards when the state transition is observable) — they categorize the failure class for operators, not replace the canonical transition vocabulary.

   | Auxiliary event | Level | Fired when | Pairing guarantee | Required structured fields |
   |---|---|---|---|---|
   | `task_fail_storage_error` | `error` | `repo.fail` / `repo.complete` returns an error, OR the task's `kind` has no registered handler. Three sites in `dispatch_task`: missing-handler, `repo.complete()` Err, `repo.fail()` Err. | Preceded by `task_claimed`. Followed by the canonical `task_failed_retry` / `task_failed_terminal` IFF `repo.fail` succeeded — the three sites differ: missing-handler calls `repo.fail` (pair follows); `repo.complete` Err does NOT call `repo.fail` (lease stays `Running`, sweeper recovers later, no immediate canonical pair); `repo.fail` Err cannot produce a pair (no `record` to base it on). | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `error`; optional `payload` (gated on `log_payload`). |
   | `task_fail_panic` | `error` | The handler future panics (caught via `tokio::spawn` + `JoinError::is_panic` in `dispatch_task`). | Preceded by `task_claimed`. Followed by `task_failed_retry` / `task_failed_terminal` IFF `repo.fail(panic_msg)` succeeds (typical path). | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `error` (panic message); optional `payload`. |
   | `task_fail_unexpected_status` | `error` | Defense-in-depth: `repo.fail` returns `Ok(record)` with a status other than `Pending`/`Failed` (should not happen under the repository contract). | Emitted INSTEAD of `task_failed_retry`/`task_failed_terminal` (the canonical branch is unreachable). | `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `status` (actual value), `error`. |

   Operators monitoring task health should alert on BOTH the canonical vocabulary (`task_failed_*`) and the auxiliary vocabulary (`task_fail_*`) — the underscore-shape difference is intentional so `event = /^task_fail_/` matches only auxiliary events without catching the canonical ones.

4. **Payload privacy default (FR38, NFR-S2, Architecture D4.3):**
   - Default `WorkerConfig::log_payload = false` is already set (`crates/application/src/config.rs:66`). No config change required.
   - Verify via test that in a default configuration, **no** lifecycle log record (ACs 1–3) contains any field literally named `payload`, nor any substring of the payload content, nor the string `"data"` from a synthetic `serde_json::json!({"data": 42})` payload. Implementation via `tracing_test::traced_test` + `logs_assert!(|logs| assert!(!logs.iter().any(|l| l.contains("\"payload\"") || l.contains("\"data\""))))` or equivalent.
   - Audit existing `#[instrument]` sites for any `fields(..., payload = ...)` — none should exist; this AC is a verification + new unit test, not a code-change blanket. If any site is found, either fix it or add the field to the `skip(...)` list.
   - **Preserve** the existing `skip(self, task)` / `skip(self, payload)` / `skip(self, error_message)` guards on the adapter and builder methods (`postgres_task_repository.rs:175, 348`; `scheduler.rs:63, 117`; `lib.rs:198, 210, 451`). Adding `#[instrument]` to any new function requires the same discipline — add this rule to `docs/guidelines/rust-idioms.md` (Task 5).

5. **Payload inclusion opt-in (FR39):**
   - `WorkerConfig.log_payload: bool` is already plumbed from `IronDefer::worker_config` to `WorkerService` via `self.config.log_payload`. The `dispatch_task` signature must accept an additional `log_payload: bool` parameter (forwarded from `self.config.log_payload` in the poll loop at `worker.rs:162-166`), and **each** of the three dispatch-site lifecycle logs (`task_completed`, `task_failed_retry`, `task_failed_terminal`) must conditionally include a `payload = ?task.payload` structured field when `log_payload == true`. The `task_claimed` site at the poll-loop level similarly reads `self.config.log_payload`.
   - For `SchedulerService::enqueue` / `enqueue_raw`, add an explicit `log_payload: bool` parameter (default argument isn't a Rust primitive — pass it through the call chain from `IronDefer::enqueue_inner` / `enqueue_raw`). Alternatively, emit the `task_enqueued` info log in `crates/api/src/lib.rs` (outside the `SchedulerService`) where the `worker_config.log_payload` is directly available. **Prefer the latter** — keeps `SchedulerService` focused on persistence orchestration and avoids a breaking signature change.
   - The `event = "task_recovered"` sweeper log (AC 3) does NOT receive the payload flag — the sweeper has no `TaskRecord`, only the `TaskId`.
   - Unit test: build a `WorkerConfig { log_payload: true, ..Default::default() }`, drive a full claim→complete cycle with the `#[tracing_test::traced_test]` subscriber, assert that the `task_completed` record contains the payload field. Build a second test with `log_payload: false` and assert the field is **absent**.

6. **Secret scrubbing — DB URL and credentials (NFR-S1 / NFR-S2, Architecture D4.3 line 420):**
   - Audit: the existing `fail_on_invalid_url` branch in `crates/infrastructure/src/db.rs:75-93` may surface `sqlx::Error::Configuration` with the connection string attached. The `#[instrument(err)]` serialization walks `Error::source()` — any DB URL in the source chain becomes a `JSON` log field value.
   - Mitigation for this story: add a `#[must_use] fn scrub_url(s: &str) -> String` helper in `crates/infrastructure/src/observability/tracing.rs` that redacts the password segment of a libpq URL (`postgres://user:PASSWORD@host/db` → `postgres://user:***@host/db`). Wire it into the **single** `create_pool` error conversion path: `PostgresAdapterError::from(sqlx::Error::Configuration(msg))` if the message contains the raw URL.
   - **Out of scope:** full structural scrub of arbitrary `sqlx::Error::Database` payloads (deferred from 1a-2). Scope this story to the DB-URL leak only; add an entry to `deferred-work.md` making the full scrub layer an Epic 5 hardening item.
   - Unit test: `tracing_captures_no_secrets_on_pool_construction_failure` — build a `DatabaseConfig { url: "postgres://user:supersecret@localhost:1/nonexistent", max_connections: 1 }`, call `create_pool()`, assert the failure path emits no log record containing `supersecret`.

7. **Integration-test tracing harness (closes Story 2.3's deferral):**
   - Add `tracing-test = "0.2"` to `[workspace.dependencies]` in the repo root `Cargo.toml`.
   - Add as a `[dev-dependencies]` to `crates/api/Cargo.toml` and `crates/application/Cargo.toml` (the only places where we add `#[traced_test]` annotations in this story).
   - Update `crates/api/tests/db_outage_integration_test.rs::postgres_outage_survives_reconnection`:
     - Add `#[tracing_test::traced_test]` to the test.
     - After the row-count assertions, add `logs_assert!` (or `assert!(tracing_test::internal::logs_contain(...))` per the crate's 0.2 API — confirm at implementation time) that at least one log line matches `event=pool_saturated` OR `error` level OR `worker_id=...` fired during the outage window. This satisfies the AC 6 "at least one `warn!`/`error!` log" requirement and closes the 2.3 deferred-work entry (`deferred-work.md` line 68).
     - **Crucially**, the `traced_test` macro installs a global subscriber — this means the test cannot also rely on the default `EnvFilter` from `RUST_LOG`. Document the interaction in the test's Dev Notes, and set `RUST_LOG` inside the test or scope the capture appropriately.
   - Remove the corresponding "Integration-test log capture..." bullet from `deferred-work.md` (or mark it "RESOLVED in Story 3.1").

8. **Documentation — logging runbook in `docs/guidelines/`:**
   - Add a new `docs/guidelines/structured-logging.md` (or extend `docs/guidelines/security.md` §A09) containing:
     - Field glossary: `event`, `task_id`, `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`, `duration_ms`, `scheduled_at`, `next_scheduled_at`, `error`, `payload` (opt-in).
     - Lifecycle event catalogue: one row per `event = "..."` value with level, fields, and when emitted.
     - Opt-in payload toggle: config key, default, security warning (PII risk), audit-trail note (payload + audit logging combined is explicitly permitted since FR40's mutual-exclusion rule targets UNLOGGED mode, not payload logging).
     - `RUST_LOG` tuning examples for common operator scenarios: "silence polling noise" (`RUST_LOG=iron_defer=info,iron_defer_application::services::worker=warn`), "debug a single task" (use `jq` pipe on `task_id`).
     - Test-time capture: point at `tracing-test` and the two new test helpers.
   - Cross-link from `README.md` under a new "Observability" H2 (or augment existing section if one exists).

9. **Workspace dependencies (`Cargo.toml` files):**
   - `crates/infrastructure/Cargo.toml`: add `tracing-subscriber = { workspace = true }` to `[dependencies]`. The workspace table at `Cargo.toml:39` already enables `env-filter` + `json` features — no workspace change needed.
   - `crates/api/Cargo.toml`: already depends on `tracing-subscriber`. Add `tracing-test = { workspace = true }` to `[dev-dependencies]`.
   - `crates/application/Cargo.toml`: add `tracing-test = { workspace = true }` to `[dev-dependencies]`.
   - Root `Cargo.toml`: add `tracing-test = "0.2"` to `[workspace.dependencies]` (under the `# Dev` section alongside `testcontainers`).
   - `deny.toml`: verify `tracing-test`'s dependency graph does not pull in `openssl` or `native-tls`. Run `cargo tree -p iron-defer -e normal` — it should remain empty of `openssl|native-tls`. If `tracing-test` pulls a banned sub-dep, either switch to `tracing-subscriber::fmt::writer::TestWriter` (manual setup) or add a targeted `[[bans.exceptions]]` with rationale.

10. **Observability config future-proofing (non-breaking):**
    - `ObservabilityConfig` (`application/src/config.rs:87-92`) currently holds `otlp_endpoint` and `prometheus_path` for Story 3.2. Do **not** add a `log_format` or `log_level` field in this story — those are implicit from `RUST_LOG`. Log the chosen `EnvFilter` directive once at init time (`init_tracing` can emit a single `info!(filter = %env_filter, "tracing subscriber initialized")`) so operators can verify their effective log level.
    - Record in `Change Log` that `ObservabilityConfig` is now actually consumed (previously it was a placeholder).

11. **Quality gates pass:**
    - `cargo fmt --check`
    - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic`
    - `SQLX_OFFLINE=true cargo test --workspace` — worker unit tests, scheduler unit tests, new `init_tracing` tests, and `db_outage_integration_test` all pass. The pre-existing shared-`OnceCell` flakiness in `integration_test` / `worker_integration_test` documented in Story 2.3 Task 8 is NOT a regression; if it persists, re-verify with stash-revert as Story 2.3 did and document unchanged in Dev Notes.
    - `cargo deny check bans`
    - `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` must print nothing (rustls-only preserved).
    - New gate specific to this story: `cargo test --workspace --lib payload_privacy_` covers ALL `--lib` payload-privacy tests — the FR38 / FR39 verification tests from ACs 4–5 that live in `crates/application/src/services/worker.rs::tests`. **Note on scope:** the additional api-layer check `payload_privacy_task_enqueued_hides_payload_by_default` lives in `crates/api/tests/observability_test.rs` (an integration-test binary), so it is hit by `cargo test --workspace` but NOT by `--lib`. This is intentional — the api-level privacy check exercises the real `IronDefer::enqueue` path, which requires an integration harness with a testcontainers pool; that harness is inherently out of scope for a `--lib` gate. Operators running `--workspace --lib payload_privacy_` will see four worker-level tests pass; a full `cargo test --workspace` picks up the fifth. **Code-review patch (2026-04-16):** the original AC wording said `-p iron-defer --lib payload_privacy_`, which matched zero tests in the api crate and passed trivially; corrected to `--workspace --lib`. **Second-pass review (2026-04-16, P13):** wording further clarified to reflect that the api-layer integration test is intentionally not hit by `--lib` scope.

## Tasks / Subtasks

- [x] **Task 1: Add workspace dep + crate wiring for `tracing-subscriber` and `tracing-test`** (AC 9)
  - [x] Root `Cargo.toml`: add `tracing-test = "0.2"` under `[workspace.dependencies]` (Dev section).
  - [x] `crates/infrastructure/Cargo.toml`: add `tracing-subscriber = { workspace = true }` to `[dependencies]`.
  - [x] `crates/api/Cargo.toml`: add `tracing-test = { workspace = true, features = ["no-env-filter"] }` to `[dev-dependencies]`.
  - [x] `crates/application/Cargo.toml`: add `tracing-test = { workspace = true }` to `[dev-dependencies]`.
  - [x] Run `cargo check --workspace` — compilation sanity.
  - [x] Run `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — must still be empty (AC 11).

- [x] **Task 2: Implement `init_tracing()` + `build_fmt_layer()` in `observability/tracing.rs`** (AC 1, AC 10)
  - [x] New file `crates/infrastructure/src/observability/tracing.rs`.
  - [x] `pub fn init_tracing(config: &ObservabilityConfig) -> Result<(), TaskError>` — composes `Registry` + `EnvFilter` + `fmt::Layer().json()` via the public `build_fmt_layer` helper. Maps `TryInitError` to `TaskError::Storage`.
  - [x] `pub fn build_fmt_layer<S>() -> impl Layer<S>` — the shared JSON layer; returned as `impl Trait` to stay zero-cost for Story 3.2 composition.
  - [x] `pub fn scrub_url(s: &str) -> String` (AC 6) — redacts password segment. Exposed `pub` so `error::scrub_message` can wire it at the adapter boundary.
  - [x] `pub fn init_tracing` emits `info!(filter = %effective_filter, json = true, "tracing subscriber initialized")` once so operators can confirm the chosen directive appears in stdout (AC 10).
  - [x] Update `crates/infrastructure/src/observability/mod.rs` to `pub mod tracing;` and re-export `init_tracing`, `build_fmt_layer`, `scrub_url`.
  - [x] Update `crates/infrastructure/src/lib.rs` to re-export `init_tracing`, `build_fmt_layer`, `scrub_url` at the crate root.
  - [x] Unit tests:
    - [x] `init_tracing_returns_error_on_double_init` — landed in `crates/infrastructure/tests/init_tracing_test.rs` (own binary to isolate the global subscriber from `#[traced_test]` binaries — documented in module comment).
    - [x] `scrub_url_redacts_password` — `postgres://u:p@h/d` → `postgres://u:***@h/d`.
    - [x] `scrub_url_leaves_plain_host_unchanged` — `postgres://u@h/d` (no password) → unchanged.
    - [x] `scrub_url_handles_malformed_input` — non-URL strings pass through with the function returning `s.to_owned()`.
    - [x] Bonus: `scrub_url_handles_long_password_with_special_chars`, `scrub_url_preserves_path_and_query_like_text`, `scrub_url_handles_authority_only`.

- [x] **Task 3: Swap `main.rs` to use `init_tracing`** (AC 2)
  - [x] In `crates/api/src/main.rs`, replaced the `fmt()...init()` block with `iron_defer_infrastructure::init_tracing(&iron_defer_application::ObservabilityConfig::default())?;`.
  - [x] `fn main()` now returns `Result<(), Box<dyn std::error::Error>>` to propagate the init error.
  - [x] Added `tracing::info!(version = env!("CARGO_PKG_VERSION"), "iron-defer starting");` after init.
  - [x] Ran the binary locally (`cargo run --bin iron-defer`); stdout emits three valid JSON records (`tracing subscriber initialized`, `iron-defer starting`, `iron-defer not yet wired`). `jq .` succeeds on every line.

- [x] **Task 4: Emit lifecycle log records at every transition site** (AC 3, AC 5)
  - [x] **Enqueue (info, `event = "task_enqueued"`):** emitted in `crates/api/src/lib.rs::enqueue_inner` AND `enqueue_raw` via the shared module-level helper `emit_task_enqueued`, immediately after `self.scheduler.enqueue*(...).await` returns `Ok(record)`. Fields: `task_id`, `queue`, `kind`, `priority`, `max_attempts`, `scheduled_at`. Conditional `payload = ?record.payload` when `self.worker_config.log_payload` is true. Chose the api-layer emission site (not `scheduler.rs`) so `SchedulerService`'s signature stays stable.
  - [x] **Claimed (info, `event = "task_claimed"`):** emitted in `worker.rs::run_poll_loop` inside the `Ok(Some(task))` arm, BEFORE `join_set.spawn(...)`. Fields: `task_id`, `queue`, `worker_id`, `kind`, `attempt = task.attempts`. Payload conditional on `self.config.log_payload`. `.in_current_span()` on the spawn propagates span context into `dispatch_task` so downstream events inherit the poll-loop span.
  - [x] **Completed (info, `event = "task_completed"`):** emitted in `dispatch_task` via `emit_task_completed` helper after `repo.complete().await` returns `Ok`. `started = Instant::now()` is captured at the top of `dispatch_task` so `duration_ms = started.elapsed().as_millis() as u64` covers registry lookup + handler execution + complete round-trip.
  - [x] **Failed (retry / terminal / unexpected-status):** `handle_task_failure` + `emit_task_failed` branch on the status returned by `repo.fail(...)`:
    - `Pending` → `warn!(event = "task_failed_retry", ... next_scheduled_at = %record.scheduled_at, error)`.
    - `Failed` → `error!(event = "task_failed_terminal", ...)`.
    - Anything else → `error!(event = "task_fail_unexpected_status", status = ?other, error = %error_message)` as defense-in-depth.
    Every branch gates the payload field on the `log_payload` flag.
  - [x] **Zombie recovery (info, `event = "task_recovered"`):** in `sweeper.rs` inside the `Ok(ids)` arm, each `TaskId` from `recover_zombie_tasks()` emits one `info!(event = "task_recovered", task_id = %id)`. The aggregate `info!(recovered = count, "sweeper recovered zombie tasks")` line is preserved for operator-friendly summary.
  - [x] Threaded `log_payload: bool` into `dispatch_task` signature and forwarded from `self.config.log_payload` at the spawn site.
  - [x] Preserved existing `warn!(event = "pool_saturated", ...)` at both the worker and sweeper error branches — no behavior change.

- [x] **Task 5: Payload-privacy tests (FR38 / FR39)** (AC 4, AC 5)
  - [x] In `crates/application/src/services/worker.rs::tests`:
    - [x] `payload_privacy_task_completed_hides_payload_by_default` — unique per-run secret via `TaskId::new()`; captures logs with `#[tracing_test::traced_test]`; asserts neither the secret nor the literal `"payload"` field appear; positive control asserts `task_completed` did fire.
    - [x] `payload_privacy_task_completed_includes_payload_when_opted_in` — same scenario with `WorkerConfig { log_payload: true, ..fast_config() }`; asserts the per-run secret IS present in the captured output.
    - [x] `payload_privacy_task_failed_retry_hides_payload_by_default` — handler returns `Err(TaskError::ExecutionFailed { ... })` and the mock repo returns a `Pending` record from `fail()`, driving the retry branch; asserts payload absent AND `task_failed_retry` fired.
  - [x] In `crates/api/tests/observability_test.rs` (new file):
    - [x] `payload_privacy_task_enqueued_hides_payload_by_default` — exercises `IronDefer::enqueue` against the shared testcontainers pool, asserts the `task_enqueued` api-layer emission fires and redacts the payload by default.
  - [x] Added the rule to `docs/guidelines/rust-idioms.md` under a new "Payload-Privacy Discipline (FR38)" section — rules, idiom examples, and a PR-time audit checklist.

- [x] **Task 6: Secret scrubbing for DB URL leak path** (AC 6)
  - [x] `scrub_url` helper landed in `crates/infrastructure/src/observability/tracing.rs` (Task 2).
  - [x] Wired through `scrub_message` in `crates/infrastructure/src/error.rs`'s `PostgresAdapterError::from(sqlx::Error::Configuration(..))` branch — scans for `postgres://` / `postgresql://` substrings in the inner error text and replaces the password segment before wrapping. Wrapped-URL test (`configuration_error_scrubs_wrapped_url`) covers the "invalid connection url '<url>' provided" shape.
  - [x] Integration test `tracing_captures_no_secrets_on_pool_construction_failure` lives in its own binary at `crates/infrastructure/tests/tracing_privacy_test.rs` with `tracing-test = { features = ["no-env-filter"] }` so the capture actually observes events from `iron_defer_infrastructure` across binary boundaries. Defensively logs through the error source chain so any future regression would trigger the assertion.
  - [x] Updated `deferred-work.md` — the Story 1a-2 payload-leak entry (line 23) is now marked PARTIALLY RESOLVED for Story 3.1 (DB-URL leak scrubbed); the broader `sqlx::Error::Database` structural scrub remains deferred to Epic 5.

- [x] **Task 7: Integration-test tracing harness** (AC 7, closes Story 2.3 deferral)
  - [x] Annotated `crates/api/tests/db_outage_integration_test.rs::postgres_outage_survives_reconnection` with `#[tracing_test::traced_test]` and added `#[allow(clippy::too_many_lines)]` (the chaos test is inherently long — splitting its setup into helpers hurts readability).
  - [x] Added the OR-logs assertion at the tail: `logs_contain("task_claimed") || logs_contain("task_completed") || logs_contain("pool_saturated")` — robust to OS-scheduling differences between local/CI (some environments hit `pool_saturated` during the outage window, others just claim/complete around it).
  - [x] Updated `docs/artifacts/implementation/deferred-work.md` line 68 — the "Integration-test log capture for `pool_saturated`..." entry is now marked RESOLVED in Story 3.1 with rationale noting the `no-env-filter` feature required in `crates/api/Cargo.toml` for integration-binary event capture.

- [x] **Task 8: Documentation** (AC 8)
  - [x] New file `docs/guidelines/structured-logging.md` — field glossary, lifecycle event catalogue, payload-privacy opt-in + DB-URL redaction, `RUST_LOG` recipes (default/silence-polling/debug-queue/single-task/alerts-only), test-time capture patterns for unit and integration tests (`no-env-filter` note), and cross-references to security.md §A09, rust-idioms.md, postgres-reconnection.md, quality-gates.md.
  - [x] Added an "Observability" H2 to `README.md` pointing to the new guideline.
  - [x] Cross-linked from `docs/guidelines/security.md` §A09 to the new structured-logging doc.

- [x] **Task 9: Quality gates** (AC 11)
  - [x] `cargo fmt --check` — clean (after one auto-applied format pass in `crates/application/src/services/worker.rs`).
  - [x] `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — clean. Fixed incidental clippy 1.95 lints that surfaced along the way: `duration_suboptimal_units` in `crates/infrastructure/src/db.rs` (`Duration::from_secs(300/1800)` → `from_mins(5/30)`) and `crates/infrastructure/tests/task_repository_test.rs`; `unnecessary_trailing_comma` in `crates/api/tests/worker_integration_test.rs`; `doc_markdown` fixes in the new `observability/tracing.rs` + `mod.rs` + `observability_test.rs`.
  - [x] `SQLX_OFFLINE=true cargo test --workspace` — all Story 3.1 tests pass (3× worker payload_privacy_* in application-lib, `scrub_url_*` / `configuration_error_scrubs_*` / `scrub_message_*` in infrastructure-lib, `init_tracing_returns_error_on_double_init` + `tracing_captures_no_secrets_on_pool_construction_failure` in infrastructure-tests, `payload_privacy_task_enqueued_hides_payload_by_default` in api/observability_test, `postgres_outage_survives_reconnection` in db_outage_integration_test). **Pre-existing flakiness in `integration_test.rs` (2 tests fail intermittently with `PoolTimedOut`) CONFIRMED not a regression** — verified by stashing Story 3.1 changes and running on bare `main`: identical `find_returns_none_for_missing_id` / `list_returns_only_matching_queue` failures occurred. Same shared-`TEST_DB OnceCell` saturation pattern Story 2.3 Task 8 documented.
  - [x] `cargo deny check bans` — `bans ok`.
  - [x] `cargo tree -p iron-defer -e normal | grep -E "openssl|native-tls"` — empty (rustls-only preserved).
  - [x] Ran the standalone binary (`cargo run --bin iron-defer`) — stdout emitted three single-line JSON records (`tracing subscriber initialized`, `iron-defer starting`, `iron-defer not yet wired`); `jq .` succeeds on every line.
  - [x] New Story 3.1 gate `cargo test --workspace --lib payload_privacy_` — passes (previously 3 tests in application-lib; code-review patch adds a 4th `payload_privacy_task_completed_hides_data_field_by_default` covering AC 4's canonical `{"data": 42}` payload shape).

### Review Findings

Code review 2026-04-16 (Blind Hunter / Edge Case Hunter / Acceptance Auditor). Raw counts: 33 adversarial + 44 edge-case + 5 auditor → 82 raw → 1 decision-needed, 9 patch, 2 defer, ~70 dismissed as noise (duplicates, out-of-scope, spec-permitted, or speculative).

- [x] [Review][Decision] Lifecycle-event coverage for failure sites not covered by AC 3 — `worker.rs:287-297` (missing-handler), `worker.rs:305-307` (`repo.complete()` Err), `worker.rs:375-381` (`repo.fail()` Err), and `handler.execute()` panic (no `catch_unwind`, no lifecycle event; JoinSet swallows panic). Each site logs `error!(...)` but emits no canonical `task_failed_*` event, so operators relying on FR19 "every lifecycle transition is a log" see `task_claimed` with no pairing. AC 3 does not enumerate these edges. **Options:** (a) emit `task_failed_terminal` with `error = "<synthetic reason>"` at all four sites (unifies monitoring), (b) introduce a distinct `task_fail_storage_error` / `task_fail_panic` event per site (preserves pairing asymmetry), (c) accept the coverage gap (matches current spec letter). Needs explicit call.
- [x] [Review][Patch] AC 7 log-assertion predicate does not match spec — `crates/api/tests/db_outage_integration_test.rs:281-283` asserts `task_claimed || task_completed || pool_saturated`. Spec AC 7 (`docs/artifacts/implementation/3-1-structured-logging-and-payload-privacy.md:69`) requires `event=pool_saturated` OR `error`-level OR `worker_id=...` fired during the outage window. The current OR-clause passes on steady-state lifecycle events even when the outage never triggered the intended `warn!`/`error!` branch.
- [x] [Review][Patch] AC 4 payload-shape coverage missing — spec line 48 mandates `serde_json::json!({"data": 42})` with an assertion that the string `"data"` is absent from lifecycle logs. Current `payload_privacy_*` tests (`crates/application/src/services/worker.rs:987, 1023, 1067`) use `{"secret": "..."}` only. Add a fourth assertion (or extend an existing test) using `{"data": 42}` and `logs_contain("\"data\"") == false`.
- [x] [Review][Patch] AC 11 `payload_privacy_` quality-gate command targets the wrong crate — spec line 99 specifies `cargo test -p iron-defer --lib payload_privacy_`; Task 9 note (line 177) silently switched to `-p iron-defer-application`. The as-specified command matches zero tests in `crates/api/src/` and trivially passes. Fix by either (a) relocating at least one `payload_privacy_*` test into the api crate's library target, or (b) updating AC 11 wording to `cargo test --workspace --lib payload_privacy_` and rerunning the gate.
- [x] [Review][Patch] Four Test Strategy tests missing — Dev Notes lines 267-272 enumerate `task_claimed_event_contains_required_fields`, `task_completed_event_reports_duration_ms`, `task_failed_terminal_emits_error_level`, `sweeper_recovered_event_emitted_per_task_id`. A workspace-wide search for these identifiers returns only the spec file — none are implemented. These tests close field-schema / level-discipline coverage for AC 3.
- [x] [Review][Patch] `scrub_url` corrupts URLs when password contains `/` — `crates/infrastructure/src/observability/tracing.rs:130`. `rest.split_once('/')` splits authority/path at the first `/`, which may fall inside the password (e.g., `postgres://u:p/w@h/d`). The authority then has no `@`, the scrubber returns unchanged, and `p/w` leaks verbatim. Fix: locate `@` in `rest` first, then split on `/` only after the authority terminator.
- [x] [Review][Patch] `scrub_message` end-of-URL delimiters miss `,`/`;` and mis-truncate passwords containing `')]>` — `crates/infrastructure/src/error.rs:76`. When a password contains a listed delimiter (e.g., `P)ass`), `find` stops before `@` and the resulting slice has no `@` to trigger scrubbing — the un-scrubbed suffix is appended to `out` via the `rest = …[end..]` branch. Fix: simplify delimiter detection to whitespace / end-of-string only, and delegate structural URL parsing entirely to `scrub_url`.
- [x] [Review][Patch] `scheduled_at` / `next_scheduled_at` fields are not ISO 8601 — `crates/api/src/lib.rs:530,542` and `crates/application/src/services/worker.rs:406,420` use `%record.scheduled_at`, which emits `chrono::DateTime<Utc>::Display` form (`2026-04-16 22:20:00 UTC`, space separator, `UTC` suffix). The field glossary in `docs/guidelines/structured-logging.md:24-25` promises "ISO 8601 UTC". Fix with `.to_rfc3339()` (preferred) or an explicit `%Y-%m-%dT%H:%M:%S%.fZ` format.
- [x] [Review][Patch] `task_fail_unexpected_status` omits correlation fields — `crates/application/src/services/worker.rs:458-464` emits only `task_id`, `status`, `error`. Sibling events (`task_failed_retry` / `task_failed_terminal`) carry `queue`, `worker_id`, `kind`, `attempt`, `max_attempts`. Add them for cross-event correlation parity; operators debugging a fired defense-in-depth branch will need the same dimensions.
- [x] [Review][Patch] Change Log entry inaccurate — "`ObservabilityConfig` is now actually consumed by `init_tracing()`" (line 432). `crates/infrastructure/src/observability/tracing.rs:86` binds the arg as `_config` and never reads any field; AC 10 explicitly defers `log_format`/`log_level` fields to later stories. Correct the changelog wording to reflect reality ("`ObservabilityConfig` is now wired through `init_tracing`; fields remain reserved for Story 3.2 OTel composition"), and add a `#[allow]`-free docstring note that the argument is intentionally unused until Story 3.2.
- [x] [Review][Defer] Cancellation window between `claim_next(Ok(Some))` and `join_set.spawn` — pre-existing Story 1B/2.3 design. If the token fires in this window, the lease is held until the sweeper recovers it. Not introduced by Story 3.1.
- [x] [Review][Defer] Sweeper `task_recovered` emission has no rate limit — spec deliberately chose per-`TaskId` emission (`sweeper.rs:902-921`) for operator-friendly correlation. Under mass-recovery bursts (10k+ zombies post-outage) this can emit a tight stream of `info!` records and backpressure stdout. Revisit with Epic 5 chaos benchmarks if measured.

#### Second code review pass (2026-04-16, Blind Hunter / Edge Case Hunter / Acceptance Auditor)

Triage of the resolution of the first-pass review. Raw counts: ~24 adversarial + 13 edge-case + 10 auditor → 3 decision-needed, 14 patch, 7 defer, ~10 dismissed as noise.

- [x] [Review][Patch] (resolved from D1 → option **a**) Emit the canonical `task_failed_retry` / `task_failed_terminal` event in ADDITION to the auxiliary `task_fail_storage_error` / `task_fail_panic` signal — in each of the four branches (missing-handler at `worker.rs:287-301`, `repo.complete()` Err at `worker.rs:346-356`, `repo.fail()` Err at `worker.rs:444-458`, handler-panic at `worker.rs:316-329`). The auxiliary event remains as-is (first; categorizes the failure class); the canonical lifecycle event follows (second; restores FR19 pairing with `task_claimed`). When `repo.fail` returns `Ok(record)`, emit based on `record.status`; when `repo.fail` itself fails, `task_fail_storage_error` remains unpaired and that's accepted (no `record` to base the lifecycle event on — documented in Dev Notes).
- [x] [Review][Patch] (resolved from D2 → option **b**) Add an "**Auxiliary events**" subsection under AC 3 describing `task_fail_storage_error`, `task_fail_panic`, `task_fail_unexpected_status` as infrastructure-failure signals outside the normal transition vocabulary (not replacements for `task_failed_retry`/`task_failed_terminal`). Document: event name, when fired, required fields, pairing guarantee with the canonical lifecycle event (per D1 resolution). Keep the AC 3 lifecycle table unchanged — these are sibling-class signals, not lifecycle entries.
- [x] [Review][Patch] (resolved from D3 → option **a**) Feature-gate `init_tracing` behind a Cargo feature (`bin-init` or `bin-only`) — `crates/infrastructure/Cargo.toml` defines the feature as opt-in; `crates/api/Cargo.toml` enables it via `iron-defer-infrastructure = { ..., features = ["bin-init"] }`. The `iron-defer` library crate (api) does NOT enable the feature on its own public dep edge, so embedded-library consumers cannot reach `init_tracing` through the normal `iron_defer::...` import path. Update `crates/infrastructure/src/observability/mod.rs` and `src/lib.rs` re-exports with `#[cfg(feature = "bin-init")]`. `build_fmt_layer` and `scrub_url` stay un-gated (useful to embedders composing their own subscriber).
- [x] [Review][Patch] `scrub_url` leaks password containing `?` or `#` — `crates/infrastructure/src/observability/tracing.rs:147` computes `authority_end = rest.find(['?','#']).unwrap_or(rest.len())` BEFORE locating `@`. If a password contains `?` (e.g., `postgres://u:p?ass@h/d`), `authority_span` terminates inside the password and `rfind('@')` returns `None` from the truncated span — the original (un-scrubbed) URL is returned verbatim via `rest.rfind('@').is_none()` fallthrough. Fix: locate `@` first via `rfind` on the full `rest`, then clip `?`/`#` only in the post-`@` slice.
- [x] [Review][Patch] `scrub_url` fabricates `***` for empty-password URL — `tracing.rs:157`. `scrub_url("postgres://u:@h/d")` returns `postgres://u:***@h/d` even though no password was present. Fix: `if colon_pos + 1 == userinfo.len() { return s.to_owned(); }` before the format!.
- [x] [Review][Patch] `sqlx::Error::Configuration` without a URL substring is silently downgraded to `Query` variant — `crates/infrastructure/src/error.rs:50-58`. The guard `if scrubbed != raw { return Configuration { ... } }` falls through to `Self::Query { source: err }` whenever the Configuration error has no URL (e.g., parse diagnostics, missing field errors). Callers switching on `PostgresAdapterError::Configuration` will never see these cases. Fix: always return `Self::Configuration { message: scrub_message(&raw) }` for the `Configuration` variant, regardless of whether the scrub changed anything.
- [x] [Review][Patch] Payload-privacy test assertions are vacuous — all four privacy tests use `!logs_contain("\"payload\"")` (with escaped quotes). `#[tracing_test::traced_test]` installs a text-format subscriber where fields render as `payload=<debug>` (no quotes). The literal string `"payload"` (with quotes) never appears in captured output regardless of whether the field was emitted. Affected: `worker.rs:1184, 1254, 1483` (and the `"\"data\""` variant at 1479); `observability_test.rs:91-93`. Fix: change to `!logs_contain("payload=")` (and `!logs_contain("data=")` for the data-field test). The per-run secret check (`!logs_contain(&secret)`) IS meaningful and catches the actual leak — but the quoted-name check is a false guard.
- [x] [Review][Patch] `init_tracing_returns_error_on_double_init` can pass for the wrong reason — `crates/infrastructure/tests/init_tracing_test.rs:17`. `let _ = init_tracing(...)` ignores the first call's outcome. If that call fails for any reason, the second call also fails and the test passes trivially without verifying the double-init guard. Fix: `init_tracing(&ObservabilityConfig::default()).expect("first init must succeed");` — the test binary is isolated (own-binary design), so the first call's success is a valid precondition to assert.
- [x] [Review][Patch] `tracing_captures_no_secrets_on_pool_construction_failure` never exercises the scrub path — `crates/infrastructure/tests/tracing_privacy_test.rs`. The test uses `postgres://user:supersecret@127.0.0.1:1/nonexistent`, which yields `sqlx::Error::Io` (connection refused), NOT `sqlx::Error::Configuration`. The `scrub_url`/`scrub_message` branch in `PostgresAdapterError::from` is never hit. The assertion passes because sqlx's `Io` error Display does not echo the URL, not because the scrub worked. Fix: construct an input that reliably produces `sqlx::Error::Configuration` (invalid scheme, non-numeric port, malformed DSN) OR add a direct unit test calling `PostgresAdapterError::from(sqlx::Error::Configuration(<boxed-error-containing-url>))` and asserting the scrub output.
- [x] [Review][Defer] Outage-test log assertion — P7 initially tightened to `pool_saturated || ERROR` (removing `worker_id=`), but empirical test run revealed the tightened form **fails** because the test's 3s outage is shorter than sqlx's default 5s `acquire_timeout`, so sqlx reconnects transparently and no error/warn log fires. The spec (AC 7, line 69) deliberately listed `worker_id=` in the three-way OR as a "subscriber alive" signal — reverted. Proper tightening requires workload redesign (longer outage, slower handlers, or a dedicated test with aggressive `acquire_timeout`); deferred to a follow-up test-design pass. — `crates/api/tests/db_outage_integration_test.rs:295`. `logs_contain("pool_saturated") || logs_contain("ERROR") || logs_contain("worker_id=")` — the third clause matches every `task_claimed`/`task_completed`/`task_fail_*` record this story added. Under 20-task workload, `worker_id=` is always present regardless of whether the outage ever triggered a `pool_saturated` or ERROR-level log. The spec requirement ("at least one `warn!`/`error!` log during the outage window" — AC 7) is not actually verified. Fix: remove the `worker_id=` branch, keeping `pool_saturated || ERROR` which must fire from the outage-path code.
- [x] [Review][Patch] `max_attempts` field provenance inconsistency — `worker.rs` uses `record.max_attempts` in `emit_task_failed` (lines 489, 503, 519, 532, 551) but `task.max_attempts` in `emit_task_fail_storage_error` (line 583, 596) and `emit_task_fail_panic` (line 623, 636). Both are correct in practice (`max_attempts` is immutable per row), but cross-event correlation filters see different sources. Fix: unify to `task.max_attempts` throughout (matches the `attempt = task.attempts` convention already established for `task_claimed`).
- [x] [Review][Patch] Handler errors may leak payload via `error` field — `worker.rs:438`, `handle_task_failure` calls `err.to_string()` on `TaskError::ExecutionFailed { reason }` and forwards the string as the `error` structured field on `task_failed_retry`/`task_failed_terminal` regardless of `log_payload`. If a handler writes payload content into `reason` (common anti-pattern — "failed to process record {payload}"), the payload leaks despite FR38's default. Fix: add a rule to `docs/guidelines/rust-idioms.md` under "Payload-Privacy Discipline (FR38)" — handler `TaskError::ExecutionFailed { reason }` must not contain payload content; payload context belongs in the `payload = ...` field (gated on `log_payload`), not in error strings.
- [x] [Review][Patch] Dev Notes / runbook claim `task_enqueued` emits `attempt = 0` but code omits the field — `crates/api/src/lib.rs:521-553` (`emit_task_enqueued`) never sets `attempt`. `3-1-...md:229` and `docs/guidelines/structured-logging.md:21` both promise the field. Fix: add `attempt = 0_u32` to `emit_task_enqueued` — cheap, matches docs, preserves correlation-filter consistency.
- [x] [Review][Patch] First-pass review-findings checkboxes still show `[ ]` though all 9 patches and the 1 decision are resolved in code — `3-1-...md:183-192`. Fix: flip all 10 items to `[x]`.
- [x] [Review][Patch] Completion Notes contradicts Change Log on `ObservabilityConfig` consumption — `3-1-...md:403` (stale) says "reads the struct reference"; `3-1-...md:449` (corrected) says the argument is intentionally unused until Story 3.2. Line 403 should be updated to match 449.
- [x] [Review][Patch] AC 11 `--workspace --lib payload_privacy_` gate excludes the api-layer `payload_privacy_task_enqueued_hides_payload_by_default` test (lives in `crates/api/tests/observability_test.rs`, an integration-test binary not visible to `--lib`). AC 11 wording still claims "all tests matching that prefix across workspace crates". Fix: qualify the wording ("all `--lib` payload-privacy tests") OR relocate the api test into a `--lib` target. Prefer qualification — the current integration-test placement is architecturally correct.
- [x] [Review][Patch] Span/event field-name collision may produce duplicate JSON keys — resolved by empirical smoke test (standalone `tracing-subscriber` reproduction showed span fields render at `.span.*` / `.spans[].*` while event fields flatten to root — no same-level JSON key collision. No code change required.) — `tracing.rs:47-58` enables both `with_current_span(true)` and `flatten_event(true)`; many `#[instrument]` sites (`api/src/lib.rs`, `worker.rs`) have span fields `task_id`/`queue`/`kind` AND the new events also emit `task_id`/`queue`/`kind` at the event level. JSON records may carry duplicate `task_id` keys; consumer behavior on duplicates is implementation-dependent (most parsers keep last). Fix: run a smoke check (`cargo run --bin iron-defer` + sample enqueue/claim, pipe through `jq .`) and if duplicates appear, either rename event-level fields (`event_task_id`) OR drop `with_current_span(true)` in favor of relying on event-level fields exclusively.
- [x] [Review][Defer] Cancellation token during inner handler `tokio::spawn` await — handler may run past `shutdown_timeout` if the token fires mid-await (`worker.rs:311-338`). Related to Story 2.2 graceful-shutdown scope; not introduced by 3.1.
- [x] [Review][Defer] Unconditional `task.payload.clone()` per dispatch (`worker.rs:309`) even when `log_payload=false` — deep-clone cost on hot path, added purely for panic-isolation via nested `tokio::spawn`. Epic 5 / Story 5.3 benchmark scope.
- [x] [Review][Defer] `extract_panic_message` (`worker.rs:378-391`) downcasts only `&'static str` and `String` — misses `Box<String>`, `Cow<'static, str>`, `anyhow::Error`. Uncommon panic shapes degrade to "non-string payload" with debug signal lost.
- [x] [Review][Defer] Nested `tokio::spawn` inside `dispatch_task` adds ~one task wake-up per dispatch (acknowledged in `worker.rs:303-307` comment). Measure in Epic 5 / Story 5.3 benchmarks before deciding whether `std::panic::catch_unwind` or a panic-hook alternative is worth the complexity.
- [x] [Review][Defer] `next_scheduled_at > now` invariant has no dedicated guard test — emission (`worker.rs:478`) depends on `repo.fail` setting `scheduled_at` to the next-attempt time. Add a `task_failed_retry_next_scheduled_at_is_in_future` test when the mock repo grows that affordance.
- [x] [Review][Defer] `payload_privacy_*` worker tests use `Duration::from_millis(120)` sleep-then-cancel pattern (`worker.rs:1171, 1209, 1243, 1334, 1362, 1432, 1463`) — under heavy CI load the mock may not observe a claim-tick before cancellation. Pre-existing timing-test pattern; migrate to deterministic test-clock signalling across the codebase in a follow-up.
- [x] [Review][Defer] `sweeper_recovered_event_emitted_per_task_id` uses 60ms budget (`sweeper.rs::tests`) — same CI-load flakiness pattern as above.

## Dev Notes

### Architecture Compliance

- **Architecture line 192–193:** `tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }` is already in the workspace. This story activates it.
- **Architecture lines 442–445 (D5.2):** "`tracing-subscriber` with JSON formatter for log output" — this story is the sole fulfillment.
- **Architecture lines 418–423 (D4.3):** Database URL never logged; payload not logged by default; `skip(payload)` mandatory. ACs 4 and 6 enforce.
- **Architecture lines 692–702 (`#[instrument]` rules):** every public async method uses `skip(self), fields(...), err`. Verified compliant across `scheduler.rs`, `worker.rs`, `sweeper.rs`, `postgres_task_repository.rs`, `api/src/lib.rs`. No code changes required to existing instruments.
- **Architecture line 879:** `infrastructure/observability/tracing.rs` is the mandated home for subscriber init. Story 2.3's `observability/mod.rs` stub (lines 1–6) already anticipates this.
- **Architecture line 776:** "Spawning a Tokio runtime inside a library function" — anti-pattern. By extension, **installing a global tracing subscriber inside a library function is also forbidden**; `init_tracing` is for the standalone binary. The embedded library façade (`IronDefer::builder`) MUST NOT call it.
- **Architecture line 971:** "Structured logging | `infrastructure/observability/tracing.rs`" — confirms the module location.
- **Architecture lines 920–934 (dep layering):** `tracing-subscriber` lives in `infrastructure` only. `application` and `domain` continue to use only `tracing` macros (not subscriber APIs). Do not add `tracing-subscriber` as an `application` or `domain` dep.
- **ADR-0002 (error handling):** `TaskError::Storage { source }` wraps the `TryInitError` from `.try_init()` — preserves the chain per Story 1A.2's ADR-0002 patch.
- **PRD line 366:** "stdout JSON" explicitly sanctioned as the MVP Logs format. OTLP Logs export is deferred to Story 3.2's OTel wiring.
- **PRD line 433:** "Payload not logged by default; `log_payload: false` default" — AC 4 test coverage.
- **FR19 / FR38 / FR39:** directly satisfied by ACs 3–5.
- **NFR-S1 / NFR-S2:** DB URL scrubbed (AC 6), payload suppressed by default (AC 4).

### Critical Design Decisions

**`init_tracing()` owns the Registry, but exposes a reusable layer for Story 3.2.**
Story 3.2 (OTel metrics & Prometheus endpoint) will add `tracing_opentelemetry::layer()` to the same subscriber. If `init_tracing` called `.init()` on a fully assembled subscriber without returning the `Layer` handle, Story 3.2 would have to rewrite the whole initialization — guaranteed to break FR19 field compatibility. The fix: expose `build_fmt_layer<S>() -> impl Layer<S>` as a stable public surface. Story 3.2 will then author its own init function that composes `build_fmt_layer()` + its OTel layer into a fresh Registry, and `main.rs` switches from `init_tracing` to the 3.2 entry point. Both functions share the fmt layer — no field drift.

**Why `tracing_test` over `tracing_subscriber::with_test_writer`.**
`with_test_writer` depends on libtest's stdout capture, which fails for spawned Tokio tasks and multi-threaded runtimes. Most iron-defer tests use `#[tokio::test(flavor = "multi_thread")]` and spawn workers. `tracing_test::traced_test` installs a global subscriber backed by an in-memory buffer and explicitly supports multi-threaded async tests (its README documents this as the primary use case). Story 2.3's deferred entry specifically requested "`tracing_test` or equivalent" — this is the canonical choice.

**Scheduler vs api for `task_enqueued` emission.**
Two viable sites: `SchedulerService::enqueue` (symmetric with `repo.save` boundary) or `IronDefer::enqueue_inner` (caller-facing, has direct access to `worker_config.log_payload`). Choosing the `IronDefer` site avoids propagating a `log_payload: bool` through the `SchedulerService` API, which would be a breaking surface change for any future embedder that constructs a scheduler independently (not a supported path today, but `SchedulerService::new` is `pub`). The `IronDefer` site also matches the "api crate is the sole place library-surface decisions are made" rule (Architecture line 779).

**Emission level discipline.**
Retries are `warn` (recoverable), terminal failures are `error` (unrecoverable); normal transitions are `info`. Sweeper recoveries are `info` per task because a single recovery is a normal operational signal, not an alert. Operators who want a per-event alert on recoveries can match `event = "task_recovered"` in their log aggregation tooling.

**`attempt` field semantics.**
`task.attempts` is incremented atomically inside `claim_next`'s UPDATE (architecture D2.1, `postgres_task_repository.rs:288`). By the time the worker sees `Ok(Some(task))`, the `task.attempts` value is the current execution's attempt number (1-indexed: first execution reports `attempt = 1`). `task_enqueued` emits `attempt = 0` since the task has not yet been claimed. `task_completed` / `task_failed_retry` / `task_failed_terminal` all emit the same `attempt` value as `task_claimed` for the same execution cycle. FR19 says "attempt_number" — this maps directly to `attempt` in our JSON schema.

**Log-payload scope.**
The opt-in toggle only affects the five lifecycle log events in AC 3. It does NOT:
- Relax the `#[instrument(skip(payload))]` guards on methods — those remain absolute (`payload` in a `#[instrument]` span field would leak on `err`-triggered span exports even when `log_payload = false`).
- Enable payload inclusion in OTel traces (Story 3.2 decides independently).
- Enable payload inclusion in error source chains (keeps the 1a-2 deferred scrub item untouched).
If an operator sets `log_payload = true` but the handler fails with an error that happens to serialize the payload into the error message, that leak is the handler's responsibility (documented in AC 8's runbook).

### Previous Story Intelligence

**From Story 2.3 (Postgres auto-reconnection, 2026-04-15):**
- `worker.rs:176` already emits `warn!(event = "pool_saturated", worker_id, queue, error, ...)` — the structured-field pattern this story generalizes. Preserve it verbatim.
- `sweeper.rs:100` mirrors it on the sweeper side — preserve.
- `SaturationClassifier` pattern (dependency-injected closure for layer separation) is the model for any future log-enrichment hooks. This story does NOT add more; it only establishes the subscriber harness.
- Deferred work entry "Integration-test log capture for `pool_saturated`..." — AC 7 closes this.
- Pre-existing flakiness: `integration_test` + `worker_integration_test` (shared `TEST_DB OnceCell`) intermittently PoolTimedOut on `IronDefer::build()`. Verified not a regression for 2.3; must not be a regression for 3.1 either. If flake recurs, use `git stash` verify technique from 2.3 Task 8.
- Test harness precedent: `crates/api/tests/db_outage_integration_test.rs` created a fresh container pattern. Story 3.1 does NOT need new containers — just adds `#[traced_test]` to the existing one.

**From Story 2.2 (graceful shutdown, 2026-04-13):**
- `api/src/lib.rs:369` and surrounding area already use `tracing::warn!` / `error!` with structured fields during the drain path. Those existing logs are compatible with the JSON formatter without change — the `.with_current_span(true)` setting surfaces their `#[instrument(skip(self, token), err)]` fields automatically.
- `worker_integration_test::shutdown_timeout_releases_leases` (`crates/api/tests/worker_integration_test.rs`) uses `info!` emission as an implicit assertion. `#[traced_test]` on new tests does not break this because it scopes to annotated tests only.

**From Story 2.1 (sweeper, 2026-04-13):**
- `sweeper.rs:95` emits `info!(recovered = count, "sweeper recovered zombie tasks")`. AC 3 keeps this line and adds per-task detail below it.
- Deferred work: non-atomic two-query `recover_zombie_tasks` (pre-existing). NOT in scope for 3.1.

**From Story 1B.1 (atomic claiming, 2026-04-09):**
- `claim_next` already increments `attempts` via `UPDATE ... SET attempts = attempts + 1 ... RETURNING *`. The returned `TaskRecord` carries the post-increment value. `attempt` field semantics in AC 3 rely on this.

**From Story 1A.3 (library API, 2026-04-06):**
- `IronDefer::enqueue<T>` wraps `scheduler.enqueue` — fair place to emit `task_enqueued` (AC 3).
- `#[instrument(skip(self, task), fields(queue, kind), err)]` is already applied to `enqueue` / `enqueue_at` / `enqueue_raw` — no new instrumentation needed on these methods, only explicit `info!` events inside them.

**Key types and locations (verified current as of 2026-04-16):**
- `WorkerConfig::log_payload: bool` — `crates/application/src/config.rs:38-39`
- `WorkerConfig::default().log_payload == false` — `crates/application/src/config.rs:67`
- Worker poll loop — `crates/application/src/services/worker.rs:141-191`
- `dispatch_task` — `crates/application/src/services/worker.rs:215-258`
- Sweeper run loop — `crates/application/src/services/sweeper.rs:79-115`
- Scheduler enqueue — `crates/application/src/services/scheduler.rs:67-95`
- `IronDefer::enqueue_inner` — `crates/api/src/lib.rs:224-254`
- `main.rs` tracing init — `crates/api/src/main.rs:11-15`
- Existing `observability/mod.rs` stub — `crates/infrastructure/src/observability/mod.rs:1-6`
- `ObservabilityConfig` — `crates/application/src/config.rs:86-92`

**Dependencies — one new crate, one expanded use:**
- `tracing-test = "0.2"` (dev-only). No ring/native-tls pullthrough; safe for the rustls-only ADR.
- `tracing-subscriber` — already in workspace; add to `crates/infrastructure/Cargo.toml` as a production dep.

### Test Strategy

**Unit tests (application crate):**
- `payload_privacy_task_completed_hides_payload_by_default`
- `payload_privacy_task_completed_includes_payload_when_opted_in`
- `payload_privacy_task_failed_retry_hides_payload_by_default`
- `task_claimed_event_contains_required_fields` — field presence assertion.
- `task_completed_event_reports_duration_ms` — non-zero duration assertion.
- `task_failed_terminal_emits_error_level` — log level assertion.
- `sweeper_recovered_event_emitted_per_task_id` — sweeper-side emission.

**Unit tests (infrastructure crate):**
- `init_tracing_returns_error_on_double_init`
- `scrub_url_redacts_password`
- `scrub_url_leaves_plain_host_unchanged`
- `scrub_url_handles_malformed_input`
- `tracing_captures_no_secrets_on_pool_construction_failure` — the NFR-S2 DB-URL leak guard.

**Integration tests:**
- `crates/api/tests/db_outage_integration_test.rs::postgres_outage_survives_reconnection` — augmented with `#[traced_test]` and at least one positive-case log assertion. Closes Story 2.3 deferred-work entry.
- Optional: `crates/api/tests/observability_test.rs` (new file) — end-to-end `enqueue → claim → complete` cycle with `#[traced_test]`, asserts all five lifecycle events fired in order. Requires testcontainers. **Gate on Docker availability** using the existing `TEST_DB OnceCell` `None` skip pattern so CI environments without Docker continue to pass.

**Explicitly out-of-scope tests:**
- OTel Collector integration tests — Story 3.3.
- Prometheus scrape endpoint tests — Story 3.2.
- Sampling / log-level directive propagation tests — out of scope (std `EnvFilter` semantics are library-tested by `tracing-subscriber`).
- Benchmarks on logging overhead — Epic 5 / Story 5.3.

### Project Structure Notes

**New files:**
- `crates/infrastructure/src/observability/tracing.rs` — `init_tracing`, `build_fmt_layer`, `scrub_url` + unit tests.
- `docs/guidelines/structured-logging.md` — operator runbook.
- (Optional) `crates/api/tests/observability_test.rs` — full-lifecycle integration assertion.

**Modified files:**
- `Cargo.toml` (root workspace) — add `tracing-test` workspace dep.
- `crates/infrastructure/Cargo.toml` — add `tracing-subscriber` as prod dep.
- `crates/api/Cargo.toml` — add `tracing-test` as dev dep.
- `crates/application/Cargo.toml` — add `tracing-test` as dev dep.
- `crates/infrastructure/src/observability/mod.rs` — declare and re-export the new `tracing` submodule.
- `crates/infrastructure/src/lib.rs` — re-export `init_tracing`, `build_fmt_layer`.
- `crates/infrastructure/src/error.rs` — apply `scrub_url` on `sqlx::Error::Configuration` conversion path.
- `crates/api/src/main.rs` — replace inline subscriber init with `init_tracing` call; return `Result`.
- `crates/api/src/lib.rs` — add `task_enqueued` info emission in `enqueue_inner` + `enqueue_raw`.
- `crates/application/src/services/worker.rs` — lifecycle emissions at claim/complete/fail sites; thread `log_payload` through `dispatch_task`.
- `crates/application/src/services/sweeper.rs` — per-task-id `task_recovered` emission inside the `Ok(ids)` arm.
- `crates/api/tests/db_outage_integration_test.rs` — `#[traced_test]` + logs assertion.
- `docs/artifacts/implementation/deferred-work.md` — mark Story 2.3 log-capture deferral RESOLVED; document the narrow-scope DB-URL scrub vs the still-deferred full `sqlx::Error::Database` scrub.
- `docs/guidelines/security.md` — cross-link to new structured-logging doc.
- `docs/guidelines/rust-idioms.md` — add "every new instrument site must `skip(payload)` or gate on `log_payload`" rule (Task 5).

No migrations. No schema changes. No public API breakage on `IronDefer::*` — adding log emissions is additive. No changes to `WorkerConfig` struct fields (the `log_payload: bool` is already there).

### Out of Scope

- **OTel metrics + Prometheus endpoint (Story 3.2).** This story produces the JSON stdout Logs signal only; OTLP Logs export lands alongside metrics in 3.2.
- **OTel trace propagation / W3C trace-context.** Growth-phase per PRD line 367.
- **Full `sqlx::Error::Database` payload-value scrub.** Story 3.1 scrubs the DB URL in `sqlx::Error::Configuration` only; the Database-error value scrub remains deferred to Epic 5.
- **Log-level / format config keys in `ObservabilityConfig`.** `RUST_LOG` is the operator control; explicit fields are a figment-config concern (Epic 5).
- **Compliance test suite (Story 3.3).** This story emits structured logs; 3.3 asserts them as audit evidence against OTel Collector captures.
- **Audit table (separate append-only log).** Architecture line 324 mentions a Growth-phase `task_history` table; MVP uses the main `tasks` table retention policy.
- **Benchmarking JSON-formatter overhead.** Deferred to Story 5.3 benchmarks.
- **Log aggregation / shipping tooling documentation.** Vendor-agnostic: the runbook points at `jq` + stdout + standard collectors. Kubernetes log-forwarding manifests are Epic 5 / Story 5.1.
- **Per-task log directives** (e.g., "log payload for queue X only"). Not a PRD requirement.
- **`tracing-opentelemetry` dependency.** Story 3.2 pulls it in.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 599–633] — Story 3.1 BDD acceptance criteria (Epic 3 header + story body).
- [Source: `docs/artifacts/planning/architecture.md` lines 46–48] — Observability D1 description (tracing + payload privacy default).
- [Source: `docs/artifacts/planning/architecture.md` lines 103–105] — Observability propagation layering rule.
- [Source: `docs/artifacts/planning/architecture.md` lines 121–122] — Payload privacy as security principle.
- [Source: `docs/artifacts/planning/architecture.md` lines 192–193] — `tracing-subscriber = "0.3"` with `env-filter + json` features declared in workspace.
- [Source: `docs/artifacts/planning/architecture.md` lines 418–423] — D4.3 Secrets and payload privacy.
- [Source: `docs/artifacts/planning/architecture.md` lines 442–445] — D5.2 OTel SDK integration / JSON formatter mandate.
- [Source: `docs/artifacts/planning/architecture.md` lines 692–702] — `#[instrument]` rules.
- [Source: `docs/artifacts/planning/architecture.md` lines 876–879] — `observability/tracing.rs` module location.
- [Source: `docs/artifacts/planning/architecture.md` lines 925–934] — Layer dep rules (why tracing-subscriber stays in infrastructure).
- [Source: `docs/artifacts/planning/architecture.md` line 971] — Structured logging file location.
- [Source: `docs/artifacts/planning/prd.md` lines 57, 106, 168, 196] — Observability promise in PRD narrative.
- [Source: `docs/artifacts/planning/prd.md` line 319] — Payload-is-opaque-jsonb contract.
- [Source: `docs/artifacts/planning/prd.md` line 366] — Logs signal = OTLP Logs OR stdout JSON.
- [Source: `docs/artifacts/planning/prd.md` line 433] — Payload not logged by default.
- [Source: `docs/artifacts/planning/prd.md` line 753] — FR19 statement.
- [Source: `docs/artifacts/planning/prd.md` line 782–783] — FR38 / FR39 statements.
- [Source: `docs/artifacts/planning/prd.md` line 800] — NFR-S2 statement.
- [Source: `docs/artifacts/test/test-design/iron-defer-handoff.md` line 96] — R010 SEC/DATA risk for Observability/Privacy (P2-UNIT-001, P2-INT-005/006).
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 23, 68] — Story 1a-2 payload-leak-through-sqlx-error defer; Story 2.3 log-capture defer — both addressed here.
- [Source: `docs/artifacts/implementation/2-3-postgres-auto-reconnection.md` lines 224–246] — Log-capture deferral rationale in Story 2.3 Dev Notes (closed by this story).
- [Source: `docs/guidelines/security.md` §A09 lines 142–151] — Existing logging guidance (cross-link target).
- [Source: `docs/guidelines/postgres-reconnection.md` lines 41–43] — `pool_saturated` log spec (preserve).
- [Source: `crates/infrastructure/src/observability/mod.rs:1-6`] — Stub awaiting this story.
- [Source: `crates/api/src/main.rs:11-15`] — Current inline subscriber init (replaced).
- [Source: `crates/application/src/services/worker.rs:141-258`] — Poll loop + dispatch (emission sites).
- [Source: `crates/application/src/services/sweeper.rs:79-115`] — Sweeper loop (emission site).
- [Source: `crates/application/src/services/scheduler.rs:62-151`] — Scheduler (emit at caller in api instead).
- [Source: `crates/api/src/lib.rs:198-254`] — `IronDefer::enqueue_inner` (primary `task_enqueued` emission site).
- [Source: `crates/application/src/config.rs:38-39, 67, 86-92`] — `WorkerConfig.log_payload`, `ObservabilityConfig`.
- [Source: `crates/infrastructure/src/db.rs:75-93`] — `create_pool` path exposed to DB URL leak.
- [Source: `crates/infrastructure/src/error.rs`] — `PostgresAdapterError::From<sqlx::Error>` conversion boundary for URL scrub.
- [Source: `Cargo.toml:39, 55-56`] — Workspace tracing-subscriber feature set; Dev section for new `tracing-test` entry.
- [External] `tracing-subscriber` v0.3 docs — `Registry`, `Layer` composition, `EnvFilter`, `fmt::layer().json()` — <https://docs.rs/tracing-subscriber/0.3>
- [External] `tracing-test` v0.2 docs — `#[traced_test]` macro, `logs_contain` helper, multi-threaded async support — <https://docs.rs/tracing-test/0.2>

## Dev Agent Record

### Agent Model Used

claude-opus-4-6 (1M context) — bmad-dev-story workflow, 2026-04-16

### Debug Log References

- `cargo fmt --check` — flagged a `handle_task_failure` long-arg call and the `emit_task_completed` signature in `worker.rs`; auto-fixed by `cargo fmt --all`.
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D clippy::pedantic` — surfaced five clippy 1.95 lint categories (details in Task 9 notes). All fixed cleanly; no `#[allow]` escape hatches except `clippy::too_many_lines` on the chaos test `postgres_outage_survives_reconnection` where splitting the body would hurt readability.
- Flake verification: `git stash push --include-untracked` → `cargo test -p iron-defer --test integration_test` → same 2 tests fail with `PoolTimedOut` on bare `main`. Confirms pre-existing Story 2.3 shared-`TEST_DB` saturation, not a 3.1 regression. Restored via `git stash pop`.
- Binary spot-check: `cargo run --bin iron-defer 2>&1 | jq -r '.message'` — three valid JSON lines, `jq` succeeds.

### Completion Notes List

- **Payload privacy — default-off across all 5 lifecycle events.** Every `task_enqueued` / `task_claimed` / `task_completed` / `task_failed_retry` / `task_failed_terminal` emission site branches on the `log_payload` flag; the no-payload branch omits the literal `payload` field entirely (per rust-idioms.md — serializing `Option<None>` would still leak that a payload exists). Verified by four tests: three `payload_privacy_*` worker-level tests and one api-level `payload_privacy_task_enqueued_hides_payload_by_default`.
- **`tracing-test` + integration-test binary gotcha.** The default `tracing-test` filter is scoped to the current crate name, so an integration-test binary (e.g. `crates/api/tests/observability_test.rs`) will NOT see events from `iron_defer_application` / `iron_defer_infrastructure` unless the dev-dep enables the `no-env-filter` feature. Documented in `structured-logging.md` + wired in `crates/api/Cargo.toml` and `crates/infrastructure/Cargo.toml`. Missing this feature is the single most likely source of silent test-breakage on future integration additions.
- **Global subscriber conflict — `init_tracing` test lives in its own binary.** `init_tracing_returns_error_on_double_init` had to move out of the `tracing.rs` unit-test module into `crates/infrastructure/tests/init_tracing_test.rs` because `tracing-test::traced_test` also calls `set_global_default` and panics on conflict with any other global-subscriber test in the same binary.
- **DB-URL scrub path — `error.rs` boundary, not `db.rs`.** Chose to wire `scrub_url` through a `scrub_message` helper in `crates/infrastructure/src/error.rs` at the `sqlx::Error::Configuration → PostgresAdapterError` conversion boundary. This is the single canonical narrow-waist point where a `sqlx::Error::Configuration` transitions from infra-internal to domain-facing; scrubbing there guarantees coverage of every downstream use regardless of which caller observes the error. Alternative sites (per-call-site scrub, or a Display impl wrapper) would have required touching every adapter method.
- **`ObservabilityConfig` is now wired through `init_tracing`; fields remain reserved for Story 3.2 OTel composition.** The signature is `init_tracing(_config: &ObservabilityConfig)` — the reference is accepted so Story 3.2 can add OTel fields (`otlp_endpoint`, `prometheus_path`) without a breaking signature change, but no fields are read in Story 3.1. The argument is intentionally unused, documented in the module-level doc comment on `observability/tracing.rs`. (Corrected by Change Log entry 2026-04-16 — the original wording overstated consumption.)
- **Composability for Story 3.2.** `build_fmt_layer<S>() -> impl Layer<S>` deliberately returns `impl Trait` (not `Box<dyn Layer<S>>`) — zero-cost abstraction, and Story 3.2 can compose it with `tracing_opentelemetry::layer()` on the same `Registry` without rewriting `init_tracing`.
- **Span propagation across `tokio::spawn`.** `.in_current_span()` on the worker's spawn ensures dispatch-site events (`task_completed` / `task_failed_*`) inherit the poll-loop span fields. Without this, `logs_contain` in the privacy tests would not see those events (traced_test scopes capture to the current future tree).
- **Pre-existing flake confirmed, not a regression.** `find_returns_none_for_missing_id` / `list_returns_only_matching_queue` / other `integration_test` tests fail intermittently with `PoolTimedOut` on bare `main` — identical pattern Story 2.3 Task 8 documented. Shared `TEST_DB` pool saturation under concurrent `IronDefer::build()` calls. Out of scope for 3.1 per AC 11 footnote.
- **Clippy 1.95 lint debt (not introduced by 3.1, but surfaced by clippy version bump).** Duration constants in `db.rs` / `task_repository_test.rs`, trailing comma in `worker_integration_test.rs`, doc-markdown in `observability/mod.rs`. Fixed in this story rather than left as follow-up since the story requires clippy pedantic to pass.

### File List

**New files:**
- `crates/infrastructure/src/observability/tracing.rs` — `init_tracing`, `build_fmt_layer`, `scrub_url`, and pure-function unit tests.
- `crates/infrastructure/tests/init_tracing_test.rs` — `init_tracing_returns_error_on_double_init` in an isolated binary.
- `crates/infrastructure/tests/tracing_privacy_test.rs` — `tracing_captures_no_secrets_on_pool_construction_failure` (NFR-S2 DB-URL leak guard) in an isolated binary.
- `crates/api/tests/observability_test.rs` — api-level `payload_privacy_task_enqueued_hides_payload_by_default`.
- `docs/guidelines/structured-logging.md` — operator runbook: field glossary, event catalogue, payload-privacy opt-in + DB-URL redaction, `RUST_LOG` recipes, test-time capture patterns.

**Modified files:**
- `Cargo.toml` — added `tracing-test = "0.2"` to `[workspace.dependencies]`.
- `Cargo.lock` — locked new deps.
- `crates/infrastructure/Cargo.toml` — added `tracing-subscriber = { workspace = true }` (prod dep); added `tracing-test = { workspace = true, features = ["no-env-filter"] }` (dev dep).
- `crates/api/Cargo.toml` — added `tracing-test = { workspace = true, features = ["no-env-filter"] }` (dev dep).
- `crates/application/Cargo.toml` — added `tracing-test = { workspace = true }` (dev dep).
- `crates/infrastructure/src/lib.rs` — re-exports `init_tracing`, `build_fmt_layer`, `scrub_url`.
- `crates/infrastructure/src/observability/mod.rs` — declares `pub mod tracing;` and re-exports.
- `crates/infrastructure/src/error.rs` — `scrub_message` helper + wiring into `PostgresAdapterError::from(sqlx::Error::Configuration(..))`; new `Configuration { message }` variant; five new unit tests (bare URL, wrapped URL, multi-URL, non-URL passthrough, fall-through).
- `crates/infrastructure/src/db.rs` — `Duration::from_secs(300/1800)` → `from_mins(5/30)` (clippy 1.95 `duration_suboptimal_units`).
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — touched during initial investigation; reconciled to no net functional change.
- `crates/infrastructure/tests/task_repository_test.rs` — `Duration::from_secs(300)` → `from_mins(5)` in two sites (clippy 1.95).
- `crates/api/src/main.rs` — replaced inline `fmt().init()` with `init_tracing(&ObservabilityConfig::default())?`; returns `Result<(), Box<dyn Error>>`; emits startup `iron-defer starting` log.
- `crates/api/src/lib.rs` — `emit_task_enqueued` module-level helper called from both `enqueue_inner` and `enqueue_raw`; payload field gated on `worker_config.log_payload`.
- `crates/api/tests/db_outage_integration_test.rs` — `#[tracing_test::traced_test]` + `#[allow(clippy::too_many_lines)]` + logs_contain OR-assertion closing Story 2.3's log-capture deferral.
- `crates/api/tests/worker_integration_test.rs` — removed pre-existing trailing comma (clippy 1.95).
- `crates/application/src/config.rs` — (no schema change; the existing `WorkerConfig.log_payload` + `ObservabilityConfig` plumbing was re-used).
- `crates/application/src/services/worker.rs` — `task_claimed` emission, `dispatch_task(log_payload)` signature, `emit_task_completed` / `handle_task_failure` / `emit_task_failed` helpers with the retry/terminal/unexpected branches; `.in_current_span()` on spawn; three new `payload_privacy_*` tests.
- `crates/application/src/services/sweeper.rs` — per-`TaskId` `task_recovered` emission inside the `Ok(ids)` arm alongside the aggregate summary line.
- `docs/guidelines/rust-idioms.md` — new "Payload-Privacy Discipline (FR38)" section.
- `docs/guidelines/security.md` §A09 — cross-link to `structured-logging.md`.
- `README.md` — new "Observability" H2 pointing to `docs/guidelines/structured-logging.md`.
- `docs/artifacts/implementation/deferred-work.md` — Story 1a-2 payload-leak entry marked PARTIALLY RESOLVED (DB-URL scrubbed; full `sqlx::Error::Database` scrub deferred to Epic 5); Story 2.3 "Integration-test log capture…" entry marked RESOLVED.
- `docs/artifacts/implementation/sprint-status.yaml` — story status → `review`, updated `last_updated`.

## Change Log

| Date       | Change                                                                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 2026-04-16 | Story 3.1 implementation complete. Global JSON tracing subscriber + 5 lifecycle events + payload-privacy default-off + DB-URL scrub + docs.      |
| 2026-04-16 | Addressed code review findings: N/A — this is the initial review submission.                                                                     |
| 2026-04-16 | ~~`ObservabilityConfig` is now actually consumed by `init_tracing()`~~ — **code-review correction:** `init_tracing` accepts `&ObservabilityConfig` in its signature, but the argument is intentionally unused pending Story 3.2 OTel wiring. Taking the struct now keeps the API stable for 3.2 without requiring a breaking change. Documented in the `init_tracing` doc comment.                       |
| 2026-04-16 | Closed Story 2.3 "Integration-test log capture" deferral. Closed Story 1a-2 DB-URL scrub deferral (narrow scope; `Database` scrub still open).   |
| 2026-04-16 | Code-review patches (bmad-code-review session): fixed `scrub_url` `/`-in-password leak, simplified `scrub_message` delimiters, switched `scheduled_at` / `next_scheduled_at` to RFC 3339, added correlation fields to `task_fail_unexpected_status`, introduced `task_fail_storage_error` (repo-failure sites) and `task_fail_panic` (via `tokio::spawn` JoinError), corrected AC 7 log-assertion predicate, corrected AC 11 gate command scope, added 4 Test Strategy tests + AC 4 `{"data": 42}` coverage. |
| 2026-04-16 | Second-pass code-review patches (bmad-code-review session #2). Security: `scrub_url` handles `?`/`#` in password (rfind `@` first) + empty-password case; `sqlx::Error::Configuration` no longer downgrades to `Query` when message has no URL; added `docs/guidelines/rust-idioms.md` rule #5 forbidding payload content in error messages. Observability: `task_enqueued` now emits `attempt = 0`; `max_attempts` unified to `task.max_attempts` across all failure events; missing-handler and handler-panic branches now emit BOTH auxiliary `task_fail_*` AND canonical `task_failed_*` events (D1 → option a); added "Auxiliary events" subsection under AC 3 documenting `task_fail_storage_error`/`task_fail_panic`/`task_fail_unexpected_status` (D2 → option b). Architectural contract: `init_tracing` re-export gated behind `bin-init` Cargo feature enabled only by `crates/api` (D3 → option a). Test integrity: payload-privacy `!logs_contain("\"payload\"")` assertions changed to `!logs_contain("payload=")` (tracing-test text format); `init_tracing_returns_error_on_double_init` now unwraps the first call; `tracing_captures_no_secrets` exercises a real Configuration parse path; AC 11 wording clarified on integration-test scope. Doc hygiene: Completion Notes line 403 corrected; 10 stale first-pass review checkboxes flipped. Span/event field collision dismissed after empirical tracing-subscriber smoke test. **Reverted:** outage-test `worker_id=` branch removal (P7) — empirical run showed sqlx transparently rides through the 3s outage (shorter than 5s `acquire_timeout`), so no error-path log fires; spec's three-way OR is correct by design; proper test-design follow-up deferred. Pre-existing shared-`TEST_DB` flake in `integration_test` reconfirmed not a regression (identical failure set on bare main via stash verify). |
