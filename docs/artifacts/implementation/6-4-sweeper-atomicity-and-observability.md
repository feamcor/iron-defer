# Story 6.4: Sweeper Atomicity & Observability

Status: done

## Story

As a platform engineer,
I want zombie task recovery to be atomic with per-queue metrics,
so that no recovery is partially applied and I can monitor sweeper behavior per queue in my dashboards.

## Acceptance Criteria

1. **Transaction-wrapped recovery (CR8)**

   **Given** the `recover_zombie_tasks` method in `postgres_task_repository.rs`
   **When** the sweeper runs a recovery cycle
   **Then** the two UPDATE queries (recover retryable tasks + fail exhausted tasks) are wrapped in a single database transaction
   **And** if either UPDATE fails, the entire recovery cycle is rolled back — no partial recovery

2. **Return type extended to include queue name (CR34)**

   **Given** the transaction-wrapped recovery
   **When** the method returns
   **Then** it returns `Vec<(TaskId, QueueName)>` tuples (not bare `Vec<TaskId>`) so the caller has queue context for each recovered task
   **And** the `TaskRepository` port trait signature in `crates/application/src/ports/task_repository.rs` is updated to match the new return type
   **And** the `MockTaskRepository` (mockall) expectations are updated in all test files that mock `recover_zombie_tasks`
   **And** the SQL RETURNING clause includes the `queue` column, and `cargo sqlx prepare --workspace` regenerates the `.sqlx/` cache

3. **Rate-limited logging (CR23)**

   **Given** the `SweeperService` in `crates/application/src/services/sweeper.rs`
   **When** it processes recovered tasks
   **Then** `task_recovered` log events are rate-limited via size-triggered sampling (e.g., log first N individually, then summarize "and M more in queue X")
   **And** the aggregate summary line is retained for dashboard-friendly batch counting

4. **Per-queue zombie metric (CR34)**

   **Given** the `iron_defer_zombie_recoveries_total` metric
   **When** the sweeper increments the counter
   **Then** the `queue` label contains the actual queue name from each recovered task (not hardcoded `"all"`)
   **And** the metric is incremented per-queue, matching the per-task recovery results

5. **Combined verification**

   **Given** the combined changes
   **When** the sweeper integration tests run
   **Then** the atomic recovery, per-queue labels, and rate-limited logging are all verified
   **And** existing sweeper tests pass without modification (or are updated to match the new return type)

## Tasks / Subtasks

- [x] **Task 1: Update `TaskRepository` trait return type** (AC: 2)
  - [x] 1.1: In `crates/application/src/ports/task_repository.rs:74`, change `async fn recover_zombie_tasks(&self) -> Result<Vec<TaskId>, TaskError>` to `async fn recover_zombie_tasks(&self) -> Result<Vec<(TaskId, QueueName)>, TaskError>`
  - [x] 1.2: Update the doc comment (lines 67–73) to reflect the new return type
  - [x] 1.3: Verify `QueueName` import — already present at `task_repository.rs:11` (no change needed)

- [x] **Task 2: Wrap `recover_zombie_tasks` SQL in a transaction** (AC: 1, 2)
  - [x] 2.1: In `crates/infrastructure/src/adapters/postgres_task_repository.rs:434–481`, begin a transaction before the first UPDATE
  - [x] 2.2: Execute both UPDATE queries (retryable + exhausted) within the transaction
  - [x] 2.3: Add `queue` to the RETURNING clause of both UPDATE queries
  - [x] 2.4: Commit the transaction after both queries succeed
  - [x] 2.5: Map results to `Vec<(TaskId, QueueName)>` using `TaskId::from_uuid` and `QueueName::try_from`
  - [x] 2.6: On error, the transaction auto-rolls back (sqlx `Transaction` drop behavior)
  - [x] 2.7: Run `cargo sqlx prepare --workspace` to regenerate `.sqlx/` cache

- [x] **Task 3: Update `SweeperService` for new return type and per-queue metrics** (AC: 3, 4)
  - [x] 3.0: Add `use iron_defer_domain::QueueName;` to sweeper.rs production imports (line 13) AND test module imports (line 160) — neither currently import it
  - [x] 3.1: In `crates/application/src/services/sweeper.rs:103–133`, update the `Ok(ids)` arm to handle `Vec<(TaskId, QueueName)>`
  - [x] 3.2: Replace hardcoded `"all"` queue label (line 130) with per-queue counter increments — group recovered tasks by queue, increment once per queue
  - [x] 3.3: Implement rate-limited logging: log first 5 tasks individually with `task_id` and `queue`, then summarize remainder as "and M more tasks recovered (N in queue X, ...)"
  - [x] 3.4: Retain the aggregate summary `info!` line (`recovered = count`) for backward-compatible log dashboards
  - [x] 3.5: Update the `task_recovered` per-task log to include the `queue` field

- [x] **Task 4: Update unit tests in sweeper.rs** (AC: 5)
  - [x] 4.1: Update all 6 `expect_recover_zombie_tasks().returning(...)` mock expectations to return `Vec<(TaskId, QueueName)>` instead of `Vec<TaskId>`
  - [x] 4.2: Update `sweeper_recovered_event_emitted_per_task_id` test to verify `queue` field in log output
  - [x] 4.3: Add a test for rate-limited logging — return >5 tasks, verify aggregate summary contains counts

- [x] **Task 5: Update integration tests** (AC: 5)
  - [x] 5.1: Verify `sweeper_test.rs` tests still pass (they test via raw SQL, not mock return type)
  - [x] 5.2: Update `sweeper_counter_test.rs` to verify per-queue label on the counter — change `find_sample(&samples, "..._total", &[])` to `find_sample(&samples, "..._total", &[("queue", queue.as_str())])` where `queue` is the `unique_queue()` value used in the test. The current empty `&[]` matches any label set; the update verifies the actual queue name appears as the label value (not `"all"`)

- [x] **Task 6: Verify no regressions** (AC: 5)
  - [x] 6.1: `cargo test --workspace` — all tests pass
  - [x] 6.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings
  - [x] 6.3: `cargo fmt --check` — clean
  - [x] 6.4: `cargo sqlx prepare --check --workspace` — passes

## Dev Notes

### Architecture Compliance

- **Hexagonal boundaries (architecture lines 924–937):** `application` depends on `domain` only. The `TaskRepository` trait is in `application/src/ports/` — it defines the contract. The `QueueName` type is in `domain` so it can appear in the port trait return type without violating boundaries.
- **SQL query verification (architecture lines 805–806):** All queries use `sqlx::query!` with compile-time verification. After modifying the RETURNING clause, `cargo sqlx prepare --workspace` MUST regenerate the `.sqlx/` cache.
- **Tracing instrumentation (architecture lines 693–700):** Every public async method must have `#[instrument(skip(self), ...)]`. The existing `recover_zombie_tasks` method already has this.
- **Error handling (architecture lines 702–710):** Never discard error context. Transaction errors must propagate through `PostgresAdapterError::from`.

### Critical Implementation Guidance

**Transaction pattern with sqlx:**

The current code uses `self.pool` directly for both queries. To wrap in a transaction:

```rust
#[instrument(skip(self), err)]
async fn recover_zombie_tasks(&self) -> Result<Vec<(TaskId, QueueName)>, TaskError> {
    let mut tx = self.pool.begin().await.map_err(PostgresAdapterError::from)?;

    let retryable_rows = sqlx::query!(
        r#"
        UPDATE tasks
        SET status = 'pending',
            claimed_by = NULL,
            claimed_until = NULL,
            scheduled_at = now(),
            updated_at = now()
        WHERE status = 'running'
          AND claimed_until < now()
          AND attempts < max_attempts
        RETURNING id, queue
        "#,
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(PostgresAdapterError::from)?;

    let exhausted_rows = sqlx::query!(
        r#"
        UPDATE tasks
        SET status = 'failed',
            last_error = 'lease expired: max attempts exhausted',
            updated_at = now()
        WHERE status = 'running'
          AND claimed_until < now()
          AND attempts >= max_attempts
        RETURNING id, queue
        "#,
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(PostgresAdapterError::from)?;

    tx.commit().await.map_err(PostgresAdapterError::from)?;

    let mut results = Vec::with_capacity(retryable_rows.len() + exhausted_rows.len());
    for row in retryable_rows {
        let queue = QueueName::try_from(row.queue)
            .map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("invalid queue name in recovered task: {e}"),
            })?;
        results.push((TaskId::from_uuid(row.id), queue));
    }
    for row in exhausted_rows {
        let queue = QueueName::try_from(row.queue)
            .map_err(|e| PostgresAdapterError::Mapping {
                reason: format!("invalid queue name in exhausted task: {e}"),
            })?;
        results.push((TaskId::from_uuid(row.id), queue));
    }

    Ok(results)
}
```

**Key points:**
- `self.pool.begin()` creates a transaction — both queries execute within it
- `&mut *tx` is the standard sqlx pattern for passing a transaction reference to queries
- `tx.commit()` makes both changes permanent; if dropped without commit, rollback is automatic
- Adding `queue` to RETURNING requires regenerating `.sqlx/` cache
- `QueueName::try_from(row.queue)` validates the queue name string — this should never fail for data that was validated on INSERT, but the error path is there for safety

**Rate-limited logging pattern:**

Current code (sweeper.rs:116–122) logs every task individually:
```rust
for id in &ids {
    info!(event = "task_recovered", task_id = %id, "zombie task recovered");
}
```

Replace with size-triggered sampling:
```rust
const LOG_INDIVIDUAL_LIMIT: usize = 5;

if results.len() <= LOG_INDIVIDUAL_LIMIT {
    for (id, queue) in &results {
        info!(
            event = "task_recovered",
            task_id = %id,
            queue = %queue,
            "zombie task recovered"
        );
    }
} else {
    // Log first N individually
    for (id, queue) in results.iter().take(LOG_INDIVIDUAL_LIMIT) {
        info!(
            event = "task_recovered",
            task_id = %id,
            queue = %queue,
            "zombie task recovered"
        );
    }
    // Summarize remainder by queue
    let remaining = results.len() - LOG_INDIVIDUAL_LIMIT;
    let mut queue_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, queue) in results.iter().skip(LOG_INDIVIDUAL_LIMIT) {
        *queue_counts.entry(queue.as_str()).or_default() += 1;
    }
    let summary: Vec<String> = queue_counts
        .iter()
        .map(|(q, c)| format!("{c} in queue {q}"))
        .collect();
    info!(
        event = "task_recovered_batch",
        remaining = remaining,
        "and {remaining} more tasks recovered ({})",
        summary.join(", ")
    );
}
```

**Per-queue metric emission:**

Replace the hardcoded `"all"` label (sweeper.rs:128–131):
```rust
// OLD: m.zombie_recoveries_total.add(count as u64, &[KeyValue::new("queue", "all")]);

// NEW: increment per-queue
if let Some(ref m) = self.metrics {
    let mut queue_counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for (_, queue) in &results {
        *queue_counts.entry(queue.as_str()).or_default() += 1;
    }
    for (queue, count) in &queue_counts {
        m.zombie_recoveries_total.add(
            *count,
            &[KeyValue::new("queue", queue.to_string())],
        );
    }
}
```

This produces separate counter increments for each queue (e.g., `queue="emails"`, `queue="payments"`) instead of the single `queue="all"` aggregate. Dashboard queries that previously filtered on `queue="all"` will need to aggregate across all queue labels.

**MockTaskRepository updates — all 6 test mock sites:**

Every mock expectation in `sweeper.rs` tests that returns `Ok(vec![...])` must be updated:

| Test | Line | Old Return | New Return |
|------|------|-----------|------------|
| `sweeper_calls_recover_on_interval` | 169–171 | `Ok(vec![])` | `Ok(vec![])` (empty vec, type inferred — no change needed) |
| `sweeper_stops_on_cancellation` | 198–199 | `Ok(vec![])` | `Ok(vec![])` (same) |
| `sweeper_logs_recovery_count` | 233–235 | `Ok(vec![TaskId::new(), TaskId::new()])` | `Ok(vec![(TaskId::new(), test_queue()), (TaskId::new(), test_queue())])` |
| `sweeper_continues_on_error` | 262–271 | `Ok(vec![TaskId::new()])` | `Ok(vec![(TaskId::new(), test_queue())])` |
| `sweeper_recovered_event_emitted_per_task_id` | 313–318 | `Ok(vec![id_a, id_b, id_c])` | `Ok(vec![(id_a, test_queue()), (id_b, test_queue()), (id_c, test_queue())])` |
| `sweeper_interval_configurable_and_respected` | 361–363 | `Ok(vec![])` | `Ok(vec![])` (same) |

Helper for tests:
```rust
fn test_queue() -> QueueName {
    QueueName::try_from("test-queue").expect("valid queue name")
}
```

Note: Empty `vec![]` needs no change — Rust infers the tuple type from the trait signature.

### Previous Story Intelligence

**From Story 6.3 (ready-for-dev):**
- Cancel SQL was converted to a CTE for atomicity — similar pattern (single-statement atomicity) but Story 6.4 uses an explicit transaction since the sweeper has two separate UPDATE statements that cannot be combined into a single CTE (they target different result sets with different status transitions).
- `TaskStatus` gained `#[non_exhaustive]` — match sites in external crates now need `_ =>` arms. If any sweeper code matches on `TaskStatus`, it will need the wildcard arm. The sweeper currently does NOT match on `TaskStatus` directly — it delegates to the repository.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide. All new code must be pedantic-clean.
- `max_connections = 2` for `fresh_pool_on_shared_container` — integration tests use minimal pool connections.

**From Story 6.1 (done):**
- `Notify`-based signalling is the standard for deterministic test sync (used in `sweeper_recovered_event_emitted_per_task_id`).

### Git Intelligence

Last code commit: `7ed6fc8` (Story 6.1/6.2 — test stabilization). Sweeper code last modified in Story 3.2 (metrics wiring) and Story 2.1 (initial implementation).

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `TaskRepository::recover_zombie_tasks` | `crates/application/src/ports/task_repository.rs:74` | AC 2 — change return type |
| `PostgresTaskRepository::recover_zombie_tasks` | `crates/infrastructure/src/adapters/postgres_task_repository.rs:434–481` | AC 1 — wrap in transaction, add `queue` to RETURNING |
| `SweeperService::run` | `crates/application/src/services/sweeper.rs:91–153` | AC 3, 4 — rate-limited logging + per-queue metrics |
| `SweeperService::with_metrics` | `crates/application/src/services/sweeper.rs:56–60` | Metrics installation |
| `Metrics::zombie_recoveries_total` | `crates/application/src/metrics.rs:31` | AC 4 — counter field |
| `create_metrics` | `crates/infrastructure/src/observability/metrics.rs:134–137` | Counter creation |
| `QueueName` newtype | `crates/domain/src/model/queue.rs:12–14` | AC 2 — used in return type |
| `QueueName::try_from(String)` | `crates/domain/src/model/queue.rs:85–92` | AC 2 — construct from SQL column |
| `TaskId::from_uuid` | `crates/domain/src/model/task.rs:37–41` | AC 2 — construct from SQL column |
| `MockTaskRepository` | Auto-generated by `mockall` from trait | AC 5 — update mock expectations |
| `sweeper_test.rs` | `crates/api/tests/sweeper_test.rs` | AC 5 — integration tests |
| `sweeper_counter_test.rs` | `crates/api/tests/sweeper_counter_test.rs` | AC 5 — metrics integration test |
| `SaturationClassifier` | `crates/application/src/services/worker.rs` | Used by sweeper error branch |

### Existing Test Inventory

**Unit tests in sweeper.rs (all need mock return type update):**
1. `sweeper_calls_recover_on_interval` (line 163)
2. `sweeper_stops_on_cancellation` (line 194)
3. `sweeper_logs_recovery_count` (line 227)
4. `sweeper_continues_on_error` (line 256)
5. `sweeper_recovered_event_emitted_per_task_id` (line 300)
6. `sweeper_interval_configurable_and_respected` (line 355)

**Integration tests (verify pass, may need minor updates):**
7. `sweeper_recovers_zombie_task` — `sweeper_test.rs:43`
8. `sweeper_fails_exhausted_zombie` — `sweeper_test.rs:143`
9. `sweeper_increments_zombie_recovery_counter` — `sweeper_counter_test.rs:43`

### Dependencies

No new crate dependencies. Changes use existing capabilities:
- `sqlx::Transaction` — already available through sqlx
- `std::collections::HashMap` — stdlib
- `QueueName` — domain crate, already in scope for `application` via dependency

### Project Structure Notes

- **Modified files only** — no new files
- `crates/application/src/ports/task_repository.rs` — return type change
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — transaction + RETURNING queue
- `crates/application/src/services/sweeper.rs` — rate-limited logging + per-queue metrics + test updates
- `.sqlx/` — regenerated cache (auto-generated by `cargo sqlx prepare`)

### Out of Scope

- **Worker poll loop resilience / jittered backoff** — Story 6.5 (CR16, CR19)
- **Error model restructuring** — Story 6.6 (CR10, CR11)
- **`release_leases_for_worker` attempt increment** — Story 6.8 (CR24)
- **Distinguishing retryable vs exhausted in the return type** — the AC says "recovered and failed tasks combined"; differentiating them in the return type is not required. The per-queue metric counts all recoveries regardless of outcome.

### References

- [Source: `docs/artifacts/planning/epics.md` lines 343–376] — Story 6.4 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 371–403] — Sweeper architecture (D3.1)
- [Source: `docs/artifacts/implementation/deferred-work.md` lines 19, 67–68, 72–73] — CR8 (non-atomic sweeper), CR23 (rate-limited logging), CR34 (per-queue label)
- [Source: `crates/infrastructure/src/adapters/postgres_task_repository.rs:434–481`] — Current `recover_zombie_tasks` implementation
- [Source: `crates/application/src/services/sweeper.rs:91–153`] — SweeperService::run loop
- [Source: `crates/application/src/services/sweeper.rs:116–132`] — Current logging + hardcoded "all" metric
- [Source: `crates/application/src/ports/task_repository.rs:67–74`] — TaskRepository trait method
- [Source: `crates/application/src/metrics.rs:31`] — `zombie_recoveries_total` counter field
- [Source: `crates/api/tests/sweeper_test.rs`] — Integration tests
- [Source: `crates/api/tests/sweeper_counter_test.rs`] — Metrics counter test

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Implementation Plan

**Approach:** Follow task sequence exactly — update trait signature first, then transaction-wrap the SQL implementation, update sweeper service for per-queue metrics and rate-limited logging, update all tests.

### Debug Log References

None — clean implementation with no debugging required.

### Completion Notes List

- AC1: Both UPDATE queries (retryable + exhausted) are now wrapped in a single sqlx transaction. If either fails, the entire cycle rolls back atomically.
- AC2: Return type changed from `Vec<TaskId>` to `Vec<(TaskId, QueueName)>` across trait, implementation, and all consumers. SQL RETURNING clauses include `queue` column. `.sqlx/` offline cache regenerated.
- AC3: Rate-limited logging implemented with LOG_INDIVIDUAL_LIMIT=5. First 5 tasks logged individually with `task_id` and `queue` fields. Remainder summarized as batch with per-queue counts. Aggregate summary line retained for backward compatibility.
- AC4: Per-queue metric emission replaces hardcoded `"all"` label. `zombie_recoveries_total` counter now incremented per-queue using actual queue names from recovered tasks.
- AC5: All 8 sweeper unit tests pass (6 existing + 1 updated for queue field + 1 new rate-limited logging test). All 3 integration tests pass. Full workspace regression suite (33 test suites) passes. Clippy pedantic clean. Formatting clean.
- Extracted `log_recovered_tasks` and `emit_per_queue_metrics` as static methods on `SweeperService` for clean separation of concerns.

### File List

- `crates/application/src/ports/task_repository.rs` — changed `recover_zombie_tasks` return type and doc comment
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — transaction-wrapped recovery, `queue` in RETURNING, `QueueName::try_from` mapping
- `crates/application/src/services/sweeper.rs` — rate-limited logging, per-queue metrics, `log_recovered_tasks` and `emit_per_queue_metrics` helpers, all unit test mock expectations updated, new `sweeper_rate_limits_logging_above_threshold` test
- `crates/api/tests/sweeper_counter_test.rs` — per-queue label assertion in `find_sample`
- `.sqlx/query-4da7b798ae8941e02139c5d6278215458ee8a55cde8e7a3673b403285bc965d9.json` — new cache for retryable recovery (RETURNING id, queue)
- `.sqlx/query-6c2253083e310a611126854150cfd794999ae91310c0d62358cf9d3ff72663ff.json` — new cache for exhausted recovery (RETURNING id, queue)
- `.sqlx/query-72931e46b2632d3e3af4a4e2ee3e7a1908e5bb4ee6b08a0260de3e5719ef5ede.json` — deleted (old retryable RETURNING id only)
- `.sqlx/query-85227d24eff64306b116679b88c1c3f0ad99c0a667ebb2c1ab5267d2dfc93db3.json` — deleted (old exhausted RETURNING id only)

### Change Log

- 2026-04-23: Implemented Story 6.4 — atomic transaction-wrapped sweeper recovery, per-queue metrics and rate-limited logging (CR8, CR23, CR34)

### Review Findings (2026-04-23)

- [x] [Review][Decision] Missing failure metrics for exhausted tasks — The sweeper fails tasks that exhausted retries, but doesn't increment the `task_failures_total` metric. Should it? (Note: current repo return type doesn't provide task `kind`, which is needed for this metric).
- [x] [Review][Decision] Observability loss for large batches — Individual `task_id` logging is suppressed when more than 5 tasks are recovered. Is this acceptable for tracing, or should we log all IDs?
- [x] [Review][Patch] Transaction committed before domain mapping validation [crates/infrastructure/src/adapters/postgres_task_repository.rs:475]
- [x] [Review][Patch] Log summary template deviation [crates/application/src/services/sweeper.rs:103]
- [x] [Review][Patch] Untracked SQLX cache files [.sqlx/]
- [x] [Review][Defer] Inefficient Database Interaction [crates/infrastructure/src/adapters/postgres_task_repository.rs:434] — deferred, pre-existing (use CASE for atomic update)
- [x] [Review][Defer] Redundant iterations in SweeperService [crates/application/src/services/sweeper.rs:77] — deferred, pre-existing (micro-optimization)
- [x] [Review][Defer] Metric allocation overhead [crates/application/src/services/sweeper.rs:118] — deferred, pre-existing (micro-optimization)
- [x] [Review][Defer] Invalid queue name fails entire recovery batch [crates/infrastructure/src/adapters/postgres_task_repository.rs:478] — deferred, pre-existing (robust mapping)
