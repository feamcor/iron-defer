# Deferred Work Log

> Swept 2026-04-23. Already-resolved items from Epics 4–6 removed. Remaining items assigned to target epic/timing.

---

## Epic 4 (Operator Interface)

- **Caller-supplied idempotency keys** — `save()` is INSERT-only; no upsert for HTTP request deduplication. `IronDefer::enqueue` generates fresh `TaskId` internally so duplicate-id is unreachable from the library API. Address when REST API deduplication is needed. *Origin: Story 1A.2 review.*

---

## Epic 5 (Production Readiness)

### Configuration & Validation

- **Partial migration recovery undefined** — `crates/api/src/lib.rs`. If `MIGRATOR.run` fails partway, sqlx leaves schema half-applied. No `repair()` accessor. *Origin: Story 1A.3 review.*

- **Embedded-mode callers don't inherit hardened pool defaults** — `IronDefer::builder().pool(user_pool)` accepts any pool. Story 2.3 defaults only apply via `create_pool()`. Document visibly if embedded-mode support expands. *Origin: Story 2.3 implementation.*

### Error Model

- **`TaskError::InvalidPayload` and `ExecutionFailed` remain stringly-typed** — `Storage` now carries a boxed source chain, but the other two variants use `reason: String`. Tighten when concrete source shapes emerge. *Origin: Story 1A.1 review.*

### Performance & Resilience

- **`test_before_acquire(true)` overhead not benchmarked** — One-round-trip ping per checkout. Negligible at 500ms poll but unmeasured under high throughput. Consider making configurable. *Origin: Story 2.3 implementation.*

- **`test_before_acquire(true)` ping budget can exhaust `acquire_timeout=5s` during active outage** — First post-outage claim may spuriously `PoolTimedOut`. Accepted trade-off (cold-start latency documented). *Origin: Story 2.3 review.*

- **Shutdown delayed up to 5s per worker on stuck `claim_next` during outage** — `tokio::select!` doesn't race cancellation against the claim call itself. `shutdown_timeout` (30s) absorbs this. *Origin: Story 2.3 review.*

- **Unconditional deep-clone of `task.payload` per dispatch** — `crates/application/src/services/worker.rs`. Clones `serde_json::Value` for every dispatch. Consider `Arc<serde_json::Value>` for pointer-bump clone. Benchmark-gated. *Origin: Story 3.1 review.*

- **Nested `tokio::spawn` per dispatch** — Intentional for panic capture via `JoinError::is_panic`. Benchmark before replacing with `catch_unwind`. *Origin: Story 3.1 review.*

### Security & Privacy

- **Potential payload leakage through `sqlx::Error::Database` formatting** — DB-URL leak path is scrubbed, but arbitrary `sqlx::Error::Database` payload values (from hypothetical future CHECK/UNIQUE constraints on payload content) are not. *Origin: Story 1A.2 review.*

### CI & Tooling

- **`cargo deny check advisories` not in CI quality gates** — `protobuf 2.28.0` (via `opentelemetry-proto` → `tonic`) and deprecated `opentelemetry-prometheus 0.27` would be flagged. *Origin: Story 3.2 review.*

---

## Accepted Design Decisions (Monitor, Don't Fix)

- **Non-atomic two-query `recover_zombie_tasks`** — Retryable and exhausted UPDATEs run sequentially without a transaction. Spec sanctions this; each query is independently correct and idempotent. If DB connection fails between the two, exhausted zombies remain Running for one extra sweep cycle. *Origin: Story 2.1 review.*

- **`release_leases_for_worker` does not bump `attempts`** — A task released after drain timeout returns to Pending at its original attempt count. Consistent with sweeper's `recover_zombie_tasks` pattern (pre-existing). Consider incrementing on release when retry policy is hardened. *Origin: Story 2.2 review.*

- **Cancellation window between `claim_next(Ok(Some))` and `join_set.spawn`** — Task's lease held until sweeper recovery. Pre-existing design; sweeper is the intended safety net. *Origin: Story 3.1 review.*

- **Sweeper `task_recovered` per-`TaskId` emission has no rate limit** — Under mass-recovery bursts this can backpressure stdout. Consider size-triggered sampling. *Origin: Story 3.1 review.*

---

## Test Hygiene (Low Priority)

- **`payload_privacy_*` worker tests use 120ms sleep-then-cancel** — Under heavy CI load, mock repository may not observe a claim tick before cancellation fires. Migrate to deterministic signalling. *Origin: Story 3.1 review.*

- **`shutdown_timeout_releases_leases` elapsed < 5s assertion is tight** — 4s margin after 1s timeout with Postgres round-trip may flake under CI load. Widen or remove wall-clock assertion when observed. *Origin: Story 2.2 review.*

- **Outage-test AC 7 assertion is lenient by design** — Three-way OR includes `worker_id=` because 3s outage < 5s `acquire_timeout`. Proper fix: redesign workload or add sibling test with short `acquire_timeout`. *Origin: Story 3.1 review.*

- **`await_all_terminal` diagnostics on upstream bug** — Polls until timeout rather than surfacing pointed diagnostic if `engine.list()` drops rows. *Origin: Story 3.3 review.*

- **`fresh_pool_on_shared_container` connection-cap risk under CI load** — Per-binary math is within PG ceiling. Cross-binary contention remains when `cargo test` runs all binaries in parallel. Monitor. *Origin: Story 3.3 review.*

## Deferred from: code review of story 4-1 (2026-04-21)

- **TOCTOU race in cancel SQL** — Between the failed UPDATE and the disambiguating SELECT, concurrent requests could change the task status, causing a stale error reason. Worst case is wrong 409 error code, not a correctness issue. Consider wrapping in a single CTE or transaction if accuracy becomes important. *Origin: Story 4.1 review.*

- **Readiness probe has no explicit query timeout** — `SELECT 1` relies on the pool's `acquire_timeout` but K8s probes expect sub-second responses. Consider wrapping with `tokio::time::timeout`. *Origin: Story 4.1 review.*

## Deferred from: code review of story 4-2 (2026-04-21)

- **Unbounded `offset` allows expensive full-table scans** — `offset` is `u32` with no upper cap; large values force Postgres to skip billions of rows. Consider capping at a reasonable maximum or requiring cursor-based pagination in a future story. *Origin: Story 4.2 review.*

- **Unfiltered `GET /tasks` triggers full-table COUNT** — No `queue` or `status` filters means `SELECT COUNT(*) FROM tasks` scans the entire table; expensive at scale with millions of historical tasks. Consider requiring at least one filter or adding a warning. *Origin: Story 4.2 review.*

- **`queue_statistics()` includes historical queues with all-zero counts** — Queues with only terminal tasks (completed/failed) appear with `pending: 0, running: 0`; the `/queues` response grows unboundedly over time. Consider filtering to queues with active (pending/running) tasks. *Origin: Story 4.2 review.*

- **No pagination index for `(created_at, id)`** — `list_tasks` ORDER BY `created_at ASC, id ASC` without a composite index degrades on large tables. Address when performance benchmarks are added. *Origin: Story 4.2 review.*

- **`parse_status_filter` is case-sensitive** — `"Pending"` or `"PENDING"` returns 422; adding `.to_ascii_lowercase()` before matching would be more API-consumer-friendly. *Origin: Story 4.2 review.*

## Deferred from: code review of story 5-1 (2026-04-22)

- **No startup probe on K8s deployment** — Liveness probe fires after 10s; if DB migrations are slow on first boot, Kubernetes may restart the pod in a crash loop. Add a `startupProbe` when migration timing is benchmarked. *Origin: Story 5.1 review.*
- **No pod security context or network policy** — Container runs without `runAsNonRoot`, `readOnlyRootFilesystem`, or `allowPrivilegeEscalation: false`. Production hardening not in Epic 5 AC scope. *Origin: Story 5.1 review.*
- **`.cargo/config.toml` not copied into Docker build** — Build succeeds without it, but workspace may have build flags/aliases that affect optimization. Low risk; verify if build behavior diverges. *Origin: Story 5.1 review.*

## Deferred from: code review of 8-1-architecture-reconciliation-and-engineering-standards.md (2026-04-24)

- Security risk in manual DB scrubbing: Manual scrubbing in `infrastructure/src/error.rs` might leak data if patterns change. Pre-existing from Epic 7.
- CI gate regression: Removal of `cargo audit` and coverage tools from `ci.yml`. Pre-existing state of the codebase.
- Premature optimization: Migrating payload to `Arc<Value>` (Story 7.2) adds complexity.

## Deferred from: code review of 9-1-idempotency-key-schema-and-submission.md (2026-04-24)
- Brittle coupling between SQL predicates and Rust enum states: Hardcoded strings ('completed', 'failed', etc.) in SQL bypass Rust type system.
- Disconnected metric name definitions: Name string in infra, declaration in application.

## Deferred from: code review of 10-2-append-only-audit-log.md (2026-04-24)
- Sequential Audit Inserts in Batches: Zombie recovery performs sequential inserts for audit logs. While less efficient than bulk, it preserves transactional atomicity for this story.
- Missing Pagination in Audit API: The audit endpoint lacks pagination, which may cause issues for tasks with very long retry histories.
## Deferred from: code review of 11-1 (2026-04-25)
- AuditLogEntry constructor bloat [crates/domain/src/model/audit.rs:19] — consider Builder pattern.
- env-aware timeouts in integration tests [crates/api/tests/checkpoint_test.rs:136] — consider environment-dependent latency.

## Deferred from: code review of 12-0-taskstatus-expansion-spike.md (2026-04-25)

- **Lack of signal_payload validation** — JSONB fields like signal_payload (and pre-existing payload/checkpoint) have no size or schema validation at the database or domain level, potentially allowing resource exhaustion.
- **High SQL duplication** — Column lists and mapping logic are duplicated across 12+ queries in the Postgres adapter, increasing the risk of mapping errors during schema updates.
- **Cloning of JSON blobs in API mapping** — Large JSON structures (checkpoint, signal_payload) are cloned during DTO mapping in the REST layer, which could impact memory performance under high load.
## Deferred from: code review (2026-04-25) of 12-1-hitl-suspend-resume.md\n\n- CLI status parsing converts to lowercase and matches, potentially hiding casing inconsistencies in the underlying data layer. [crates/api/src/http/handlers/tasks.rs]

## Deferred from: code review (2026-04-26)
- **Lack of Region Access Control**: No check if producer is authorized to pin tasks to specific regions (multi-tenant risk).
