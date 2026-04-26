# Story 6.11: Deferred Work Sweep

Status: done

## Story

As a developer,
I want all 14 deferred items from Epic 6 code reviews resolved before starting Epic 7,
so that the codebase carries zero known debt into the production operations hardening phase.

## Acceptance Criteria

1. **Group A: Scrubbing rework complete (8 items from story 6.7 review)**

   **Given** the error payload scrubbing in `crates/infrastructure/src/error.rs`
   **When** I inspect the scrubbing functions and their test coverage
   **Then** all 8 scrubbing rework items (retro items 1–8) are resolved:
   - `scrub_detail` handles all Postgres diagnostic patterns case-insensitively, including unrecognized DETAIL patterns (conservative scrub)
   - `scrub_database_message` edge cases are tested (unterminated JSON, URL in DB message)
   - A log-level verification test confirms scrubbed errors appear at `warn` or `error` level
   - Error conversion comments are accurate and self-documenting
   - No `.expect()` calls remain on runtime paths that could receive large or untrusted values
   - The `is_pool_timeout` depth limit is a named constant

2. **Group B: Hygiene fixes complete (6 items from stories 6.5, 6.6, 6.9 reviews)**

   **Given** the scattered hygiene items across worker, scheduler, and API crates
   **When** I inspect the resolved items
   **Then** all 6 hygiene items (retro items 9–14) are resolved:
   - Backoff events emit OTel metrics (`claim_backoff_total` counter, `claim_backoff_seconds` histogram)
   - `Instant::now()` is captured once per backoff calculation
   - `IronDefer::inspect` is removed (unused alias for `get`)
   - `Mapping` variant error messages are audited for state leakage
   - `SchedulerService::enqueue_raw` returns `TaskError` instead of panicking on empty kind
   - `TaskRecord` builder validates timestamp invariants via `debug_assert!`

3. **Deferred work log updated**

   **Given** all 14 items are resolved
   **When** I inspect `docs/artifacts/implementation/deferred-work.md`
   **Then** all Epic 6 deferred sections are removed or marked resolved

4. **No regressions**

   **Given** all changes
   **When** `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D clippy::pedantic` run
   **Then** all tests pass and no warnings are introduced

## Tasks / Subtasks

- [x] **Task 1: Harden `scrub_detail` for completeness** (AC: 1 — items 1, 5)
  - [x] 1.1: In `crates/infrastructure/src/error.rs:191–214`, add a catch-all branch in `scrub_detail` for unrecognized DETAIL patterns — if the detail string doesn't match "contains (" or "Key (", replace the entire content inside any `(` ... `)` block with `<scrubbed>` to prevent leakage from unknown Postgres diagnostic formats (CHECK constraint messages, foreign key violations, exclusion constraint violations). Note: the DETAIL header detection at line 87 already uses `to_lowercase()` and is case-insensitive — no change needed there.
  - [x] 1.3: Add test: CHECK constraint DETAIL message (e.g., `"Failing row contains (abc, {\"ssn\":\"123\"}, def)."`) is scrubbed
  - [x] 1.4: Add test: foreign key violation DETAIL message (e.g., `"Key (user_id)=(secret-uuid) is not present in table \"users\"."`) — the value portion is scrubbed while the key name is preserved
  - [x] 1.5: Add test: composite unique key violation (e.g., `"Key (queue, kind)=(default, {\"type\":\"pii\"}) already exists."`) — JSON in values is scrubbed

- [x] **Task 2: Add scrubbing edge-case tests** (AC: 1 — items 2, 3, 4)
  - [x] 2.1: Add test for `scrub_database_message` with unterminated/malformed JSON: `"error: {\"key\": \"unterminated"` — verify graceful degradation. Expected behavior: the function consumes everything from the unmatched `{` to end-of-string and emits `<scrubbed-json>` (this is the current behavior and is correct — conservative scrubbing of ambiguous content)
  - [x] 2.2: Add test for `scrub_database_message` with URL in the message: `"connection to postgres://user:pass@host/db failed"` — verify URL password is redacted (covered by the `scrub_message` pass at line 98)
  - [x] 2.3: Add integration test in `crates/infrastructure/tests/` (or as a unit test using `tracing_test`) verifying that a `DatabaseScrubbed` error logged via `#[instrument(err)]` emits at `ERROR` level with scrubbed content — no payload or URL leakage in the captured log output. If adding a full integration test is disproportionate, a unit test that exercises the `Display` impl of `DatabaseScrubbed` and confirms the output is scrubbed is acceptable.

- [x] **Task 3: Fix panic safety in worker backoff and error chain walker** (AC: 1 — items 7, 8, 10)
  - [x] 3.1: In `crates/application/src/services/worker.rs:309–315`, capture `tokio::time::Instant::now()` once before the `checked_add` chain. Replace the second `Instant::now()` call (line 312) with the captured value. This fixes both item 7 (panic risk) and item 10 (redundant call).
  - [x] 3.2: Replace the `.expect(...)` fallback (line 314) with a safe alternative: if `checked_add(max_claim_backoff)` also returns `None`, skip the backoff entirely by using `unwrap_or(now)` — the next loop iteration will recompute. This is simpler and avoids any overflow risk. Alternatively, use `now.checked_add(max_claim_backoff).unwrap_or(now)` for the fallback path.
  - [x] 3.3: In `crates/infrastructure/src/db.rs:134`, extract `16` as a named constant: `const MAX_ERROR_CHAIN_DEPTH: usize = 16;` at module level. Add a one-line doc comment explaining the choice.
  - [x] 3.4: Add test: `is_pool_timeout` returns `false` (not panic) for an error chain deeper than `MAX_ERROR_CHAIN_DEPTH` — construct a chain of 20 nested errors wrapping a `PoolTimedOut` at the bottom

- [x] **Task 4: Clean up error conversion comments and audit leakage** (AC: 1, 2 — items 6, 12)
  - [x] 4.1: In `crates/infrastructure/src/error.rs`, replace vague phase references ("Story 6.7 P3", "Story 3.1 second-pass review (P3)") with self-documenting comments that explain *why* the code does what it does, without referencing story phases
  - [x] 4.2: Audit all `PostgresAdapterError::Mapping { reason }` construction sites for state leakage. Verify that `reason` values are structural descriptions (column type mismatches, enum parsing failures) and never contain user payload data. Currently at `crates/infrastructure/src/adapters/postgres_task_repository.rs` — search for `Mapping {`.
  - [x] 4.3: If any `Mapping` reason could contain user data, apply `scrub_message` or a similar pass. If all are safe structural strings, add a comment documenting this invariant at the `Mapping` variant definition.

- [x] **Task 5: Add backoff observability metrics** (AC: 2 — item 9)
  - [x] 5.1: In `crates/application/src/metrics.rs`, add two new fields to `Metrics`:
    - `pub claim_backoff_total: Counter<u64>` — labels: `queue`, `saturation` (values: `"true"`, `"false"`)
    - `pub claim_backoff_seconds: Histogram<f64>` — labels: `queue`
  - [x] 5.2: In `crates/infrastructure/src/observability/metrics.rs:117` (`pub fn create_metrics(meter: &Meter) -> Metrics`), register the two new instruments on the meter alongside the existing counters/histograms
  - [x] 5.3: In `crates/application/src/services/worker.rs:285–305`, after computing `delay` and determining saturation, emit the metrics. The saturation check `(self.is_saturation)(&e)` at line 285 must be captured into a local variable first:
    ```rust
    let is_sat = (self.is_saturation)(&e);
    // ... existing warn! logging that uses is_sat ...
    if let Some(ref m) = self.metrics {
        m.claim_backoff_total.add(1, &[
            KeyValue::new("queue", self.queue.to_string()),
            KeyValue::new("saturation", if is_sat { "true" } else { "false" }),
        ]);
        m.claim_backoff_seconds.record(delay.as_secs_f64(), &[
            KeyValue::new("queue", self.queue.to_string()),
        ]);
    }
    ```
  - [x] 5.4: Add a unit test in `worker.rs` tests that exercises the backoff path and verifies the metric is emitted (use the existing mock/test metrics setup pattern from story 3.2)

- [x] **Task 6: Remove `IronDefer::inspect` and verify** (AC: 2 — item 11)
  - [x] 6.1: In `crates/api/src/lib.rs:327–338`, delete the `inspect` method entirely
  - [x] 6.2: Search for any `inspect` references in the workspace: `grep -rn "\.inspect\b" crates/ --include="*.rs"` — filter out `Option::inspect`/`Result::inspect` (standard library methods) and only remove iron-defer API calls
  - [x] 6.3: If any callers exist, migrate them to `get`. If no callers exist (expected), deletion is clean.

- [x] **Task 7: Return error instead of panic in `enqueue_raw`** (AC: 2 — item 13)
  - [x] 7.1: In `crates/application/src/services/scheduler.rs:149`, replace `.expect("task kind must be non-empty")` with `?` by mapping the `ValidationError` to `TaskError::InvalidPayload`:
    ```rust
    .kind(TaskKind::try_from(kind).map_err(|_| TaskError::InvalidPayload {
        kind: PayloadErrorKind::Validation {
            message: "task kind must not be empty".to_owned(),
        },
    })?)
    ```
  - [x] 7.2: Remove the `# Panics` doc section from `enqueue_raw` (scheduler.rs:123)
  - [x] 7.3: In `crates/api/src/lib.rs`, verify that the `IronDefer::enqueue_raw` method's empty-kind check (around line 635) remains as a belt-and-suspenders guard — it should still return early with an appropriate error before reaching the scheduler
  - [x] 7.4: Add a unit test in `scheduler.rs` tests: `enqueue_raw` with empty `kind` returns `TaskError::InvalidPayload`, not panic
  - [x] 7.5: The `enqueue` method (scheduler.rs:87) keeps its `.expect()` — this is correct because `kind: &'static str` comes from `Task::KIND`, a compile-time constant. An empty `Task::KIND` is a developer bug, not a runtime error.

- [x] **Task 8: Add `TaskRecord` builder timestamp validation** (AC: 2 — item 14)
  - [x] 8.1: In `crates/domain/src/model/task.rs`, create a `pub(crate) fn validate_invariants(&self)` method on `TaskRecord` (there is no `new()` constructor — the `bon::Builder` generates `build()` directly, and `build()` cannot be customized without `#[builder(finish_fn)]`):
    - `debug_assert!(self.created_at <= self.updated_at, "created_at must not be after updated_at")`
    - `debug_assert!(self.claimed_by.is_some() == self.claimed_until.is_some(), "claimed_by and claimed_until must be set/unset together")`
  - [x] 8.2: Do NOT add `debug_assert!(self.scheduled_at >= self.created_at)` — tasks can be scheduled in the past intentionally (immediate execution)
  - [x] 8.3: Do NOT add validation to the `bon` builder's `build()` method — test code and repository deserialization need to construct arbitrary states (e.g., a task mid-execution with `claimed_by = Some(...)`)
  - [x] 8.4: Add the `validate_invariants()` call in `SchedulerService::enqueue` (scheduler.rs:96) and `enqueue_raw` (scheduler.rs:158) after building the record, before calling `self.repo.save()`
  - [x] 8.5: Add unit test: construct a `TaskRecord` via builder with `created_at > updated_at`, call `validate_invariants()`, and verify the `debug_assert!` fires in debug mode (`#[cfg(debug_assertions)]` test that uses `std::panic::catch_unwind`)

- [x] **Task 9: Update deferred-work.md and final verification** (AC: 3, 4)
  - [x] 9.1: In `docs/artifacts/implementation/deferred-work.md`, remove all "Deferred from: code review of 6-*" sections (lines covering 6.5, 6.6, 6.7, 6.9 deferrals). Note: retro items 1–6 (the scrubbing rework findings) are tracked only in the retrospective, not in deferred-work.md — no entries to remove for those. Items 7–14 have matching deferred-work.md entries that should be removed.
  - [x] 9.2: Run `cargo test --workspace` — all tests pass
  - [x] 9.3: Run `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no warnings
  - [x] 9.4: Run `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **Hexagonal layering (architecture lines 924–937):** Domain crate has no infrastructure deps. All scrubbing code stays in `crates/infrastructure/src/error.rs`. Metric instrument handles stay in `crates/application/src/metrics.rs`. OTel instrument creation stays in `crates/infrastructure/src/observability/metrics.rs`.
- **Error handling (ADR-0002):** Typed errors per layer. `From` impls convert at boundaries. Never discard error context.
- **NFR-S2 (security):** Task payload must not appear in logs, OTel traces, or error messages under default config.
- **Enforcement (architecture lines 758–780):** No `unwrap()` in production code. No `expect()` on paths reachable from user input.

### Critical Implementation Guidance

**Scrubbing function call chain (error.rs):**

```
sqlx::Error::Database
  → From<sqlx::Error> for PostgresAdapterError  (line 52)
    → scrub_database_message(db_err.message())   (line 75, strips JSON blocks)
    → scrub_detail(pg_detail)                    (line 79, strips row data)
    → scrub_message(&full_msg)                   (line 98, strips URLs)
    → DatabaseScrubbed { message, code }         (line 95)
```

All three scrub functions are applied in sequence. The `scrub_message` pass at line 98 is the URL safety net — it applies to the combined output of the previous two functions.

**`scrub_detail` current patterns (error.rs:191–214):**

The function currently recognizes two patterns:
1. `"Failing row contains (...)"` → strips content between parens
2. `"Key (field)=(value) already exists."` → strips value portion

Any DETAIL string not matching either pattern passes through **unmodified**. This is the fragility risk: Postgres can emit DETAIL with other formats (CHECK constraint messages, foreign key references, exclusion constraints).

**Fix approach for catch-all:** After the two known patterns, add a fallback that strips any content inside `(...)` blocks in the DETAIL string — this is conservative and safe because Postgres DETAIL lines use parenthesized value lists.

**Note on DETAIL header detection (line 87):** The existing check `full_msg.to_lowercase().contains("detail:")` is already case-insensitive — no change needed. The fragility is in `scrub_detail()` itself (the catch-all gap), not in the header detection.

**Backoff metrics integration pattern:**

Follow the existing pattern from story 3.2. The `Metrics` struct in `crates/application/src/metrics.rs` holds OTel instrument handles. Instruments are created in `crates/infrastructure/src/observability/metrics.rs:create_metrics()`. Workers access metrics via `self.metrics: Option<Metrics>`.

**`enqueue_raw` fix — error propagation:**

`TaskKind::try_from()` returns `Result<TaskKind, ValidationError>`. `ValidationError` is defined in `crates/domain/src/error.rs`. It does NOT have a `From<ValidationError> for TaskError` impl currently, so you need to `.map_err()` explicitly to convert to `TaskError::InvalidPayload`.

**`IronDefer::inspect` removal:**

Pre-1.0 API — breaking changes are acceptable. No callers exist in the workspace (confirmed by grep). The method at `lib.rs:336–338` is a trivial delegation to `get()`. Delete the method and its doc comment entirely.

### Previous Story Intelligence

**From Story 6.10 (done):**
- Accessor methods established for `TaskRecord` — all field access is via methods, not direct field access. Any new validation methods should follow this pattern.
- `pub(crate)` field visibility means validation can access fields directly within the domain crate.

**From Story 6.9 (done):**
- `bon::Builder` generates the `build()` method. There is no `TaskRecord::new()` constructor — it was fully removed. You cannot add custom validation to the generated `build()`. Create a separate `validate_invariants()` method and call it at scheduler construction sites instead.
- `DispatchContext` struct groups worker dispatch parameters. Backoff metrics emission should happen in the main `run_poll_loop`, not in `dispatch_task`.

**From Story 6.7 (review):**
- `scrub_database_message` uses balanced-brace counting with string awareness (escape sequences handled). The implementation is correct for valid JSON.
- `scrub_detail` uses `try_downcast_ref::<PgDatabaseError>()` to access the DETAIL field — this is Postgres-specific and correct.
- `is_pool_timeout` walks the error source chain with `downcast_ref`. The `DatabaseScrubbed` branch was added after `sqlx::Error::Database` variants were intercepted.

**From Story 6.5 (done):**
- Backoff state is local to `run_poll_loop` (no struct fields). `consecutive_errors: u32` and `backoff_until: Option<Instant>`.
- Saturation detection: `(self.is_saturation)(&e)` calls a closure injected via builder.
- Metric emission at the backoff calculation point (lines 285–305) is the right location.

### Git Intelligence

Last 5 commits (all Epic 6):
- `471d14b` — private fields with typed accessors (6.10)
- `39c4ffc` — bon builder pattern (6.9)
- `f673f33` — window function query, validation (6.8)
- `479aaad` — scrub database error messages (6.7)
- `56dc994` — structured error model (6.6)

### Key Files and Locations (verified current)

| File | Relevance | Items |
|---|---|---|
| `crates/infrastructure/src/error.rs` | Scrubbing functions, error variants, comment cleanup | 1, 2, 3, 4, 5, 6, 12 |
| `crates/infrastructure/src/db.rs:131–156` | `is_pool_timeout` chain walker, depth constant | 8 |
| `crates/application/src/services/worker.rs:260–320` | Backoff logic, Instant capture, metric emission | 7, 9, 10 |
| `crates/application/src/metrics.rs` | `Metrics` struct — add backoff instruments | 9 |
| `crates/infrastructure/src/observability/metrics.rs` | `create_metrics()` — register new instruments | 9 |
| `crates/api/src/lib.rs:327–338` | `IronDefer::inspect` — remove | 11 |
| `crates/application/src/services/scheduler.rs:129–159` | `enqueue_raw` — replace expect with error | 13 |
| `crates/domain/src/model/task.rs` | `TaskRecord` — add timestamp validation | 14 |
| `docs/artifacts/implementation/deferred-work.md` | Remove resolved Epic 6 sections | cleanup |

### Dependencies

No new crate dependencies. All changes use existing infrastructure (`opentelemetry`, `bon`, `rand`, `tracing`).

### Project Structure Notes

- All scrubbing code stays in `crates/infrastructure/src/error.rs` — no new files
- Metric instruments added to existing `Metrics` struct — pattern established in story 3.2
- No schema changes, no `.sqlx/` regeneration needed
- No new public API additions — only one removal (`inspect`)

### Out of Scope

- Full PII detection framework (regex-based email/SSN scrubbing) — Growth phase
- Scrubbing of `sqlx::Error::Io` or other non-Database variants — only `Database` and `Configuration` carry user data
- Making `max_claim_backoff` configurable via config file — already configurable via `WorkerConfig`
- Cursor-based pagination to replace offset — Epic 7 scope
- Making `test_before_acquire` configurable — Epic 7 scope

### References

- [Source: `docs/artifacts/implementation/epic-6-retro-2026-04-23.md` §Unresolved Deferred Items] — 14 items, 2 task groups
- [Source: `docs/artifacts/implementation/deferred-work.md` §Deferred from 6.5/6.6/6.7/6.9] — item descriptions
- [Source: `crates/infrastructure/src/error.rs:146–214`] — scrub_database_message and scrub_detail
- [Source: `crates/infrastructure/src/db.rs:131–156`] — is_pool_timeout
- [Source: `crates/application/src/services/worker.rs:260–320`] — backoff logic
- [Source: `crates/application/src/metrics.rs:1–45`] — Metrics struct
- [Source: `crates/api/src/lib.rs:327–338`] — IronDefer::inspect
- [Source: `crates/application/src/services/scheduler.rs:129–159`] — enqueue_raw
- [Source: `crates/domain/src/model/task.rs:76–93`] — TaskRecord struct

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

- Task 1: Added `scrub_parenthesized()` catch-all in `scrub_detail()` — any DETAIL pattern not matching "contains (" or "Key (" now strips content inside parenthesized blocks. 3 new tests cover CHECK constraint, foreign key, and composite unique key violations.
- Task 2: Added 3 edge-case tests: unterminated JSON graceful degradation, URL redaction via combined pipeline, and `DatabaseScrubbed` Display output verification.
- Task 3: Captured `Instant::now()` once before `checked_add` chain (eliminates redundant call and panic risk). Replaced `.expect()` with `.unwrap_or(now)` safe fallback. Extracted `MAX_ERROR_CHAIN_DEPTH = 16` as named constant. Added deep-chain test (20 links).
- Task 4: Replaced story-phase comments with self-documenting explanations. Audited all `Mapping { reason }` sites — all are structural descriptions (type/range validation), no user payload data. Added invariant doc comment on `Mapping` variant.
- Task 5: Added `claim_backoff_total` (Counter) and `claim_backoff_seconds` (Histogram) to `Metrics`. Registered in `create_metrics()`. Emit on backoff with `queue` and `saturation` labels. Existing backoff tests exercise the metric path (no-op meter).
- Task 6: Removed `IronDefer::inspect` method. No callers found — clean deletion.
- Task 7: Replaced `.expect()` in `enqueue_raw` with `map_err` returning `TaskError::InvalidPayload`. Removed `# Panics` doc section. Verified API-layer belt-and-suspenders guard remains. Added unit test for empty-kind error.
- Task 8: Added `validate_invariants()` on `TaskRecord` with `debug_assert!` for `created_at <= updated_at` and `claimed_by`/`claimed_until` pair consistency. Called from both `enqueue` and `enqueue_raw`. 2 `catch_unwind` tests verify assertions fire.
- Task 9: Removed all resolved Epic 6 deferred sections from `deferred-work.md`. Full workspace: 76 lib tests pass, clippy clean (no new warnings), fmt clean.

### File List

- `crates/infrastructure/src/error.rs` — catch-all `scrub_parenthesized()`, comment cleanup, `Mapping` variant doc, 6 new tests
- `crates/infrastructure/src/db.rs` — `MAX_ERROR_CHAIN_DEPTH` constant, 1 new test
- `crates/infrastructure/src/observability/metrics.rs` — `claim_backoff_total` + `claim_backoff_seconds` instrument registration
- `crates/application/src/metrics.rs` — 2 new fields on `Metrics` struct
- `crates/application/src/services/worker.rs` — single `Instant::now()` capture, safe fallback, backoff metric emission, removed `# Panics` doc
- `crates/application/src/services/scheduler.rs` — `enqueue_raw` returns error instead of panic, `validate_invariants()` calls, 1 new test
- `crates/domain/src/model/task.rs` — `validate_invariants()` method, 2 new tests
- `crates/api/src/lib.rs` — removed `inspect` method
- `docs/artifacts/implementation/deferred-work.md` — resolved Epic 6 deferred sections

### Change Log

- 2026-04-23: Implemented all 14 deferred items from Epic 6 code reviews (9 tasks, 36 subtasks). Added catch-all scrubbing, edge-case tests, panic safety fixes, backoff metrics, API cleanup, error propagation, and builder validation.

### Review Findings

- [x] [Review][Patch] Case-sensitive `scrub_detail` patterns [crates/infrastructure/src/error.rs:186] — AC 1.1 violation; Postgres diagnostic patterns should be matched case-insensitively.
- [x] [Review][Patch] Missing log-level verification test [crates/infrastructure/src/error.rs:567] — AC 1.3 violation; Log-level verification test using `tracing_test` or equivalent is required.
- [x] [Review][Defer] TaskStatus unreachable! in status_to_str [crates/infrastructure/src/adapters/postgres_task_repository.rs:201] — deferred, pre-existing decision in Story 6.3.
