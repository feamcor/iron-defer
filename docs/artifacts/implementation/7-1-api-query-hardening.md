# Story 7.1: API Query Hardening

Status: done

## Story

As an operator,
I want REST API queries and CLI output to handle real-world usage safely,
so that large result sets, missing filters, and edge-case inputs don't cause performance degradation or incorrect output.

## Prerequisites

- **Story 6.11 (Deferred Work Sweep) MUST be completed before implementation.** The Epic 6 retrospective mandates all 14 deferred items be resolved before Epic 7 begins. Verify `6-11-deferred-work-sweep` is `done` in sprint-status.yaml before starting.
- Story 6.8 window function query already in place (`COUNT(*) OVER()` in `list_tasks`).
- Story 6.10 typed accessors established (`pub fn payload(&self) -> &serde_json::Value`, etc.).

## Acceptance Criteria

### AC1: Offset Parameter Capping

Given a `GET /tasks` request with an `offset` parameter,
When the offset exceeds 10,000,
Then the offset is capped at 10,000 and the response includes the capped value,
And no full-table scan occurs regardless of the offset value.

### AC2: Unfiltered Query Warning

Given a `GET /tasks` request with no `queue` or `status` filter,
When the request is processed,
Then the endpoint emits a `warn!` log (`event = "unfiltered_task_list"`) and applies a default `limit = 100` (not a rejection),
And the OpenAPI spec documents that omitting filters returns at most 100 results with a warning.

### AC3: Pagination Index

Given the `tasks` table,
When a new SQLx migration is applied,
Then a composite index `idx_tasks_pagination` on `(created_at, id)` exists,
And paginated queries use this index for efficient keyset or offset pagination.

### AC4: Queue Statistics Filtering

Given the `queue_statistics()` query,
When it returns queue statistics,
Then only queues with at least one task in a non-terminal state are included (no historical zero-count ghost queues),
And the `GET /queues` endpoint reflects only active queues.

### AC5: Case-Insensitive Status Parsing

Given a `GET /tasks?status=PENDING` request (uppercase),
When `parse_status_filter` processes the value,
Then it applies `.to_ascii_lowercase()` before matching,
And `PENDING`, `Pending`, and `pending` all return the same results.

### AC6: Pagination Bounds Defensive Checking

Given the CLI `output.rs` pagination display,
When `offset + 1 > total` (e.g., offset=100, total=50),
Then the pagination bounds are clamped defensively (no arithmetic overflow or panic),
And the output displays "showing 0 of 50 tasks" (or equivalent clear message).

### AC7: SQLx Cache Regeneration

Given the `.sqlx/` offline cache,
When the new migration and query changes are applied,
Then `cargo sqlx prepare --workspace` regenerates the cache,
And `cargo sqlx prepare --check --workspace` passes in CI.

## Tasks / Subtasks

- [x] Task 1: Offset capping in REST handler (AC: #1)
  - [x] Add `MAX_OFFSET: u32 = 10_000` constant to `crates/api/src/http/handlers/tasks.rs`
  - [x] Change line 313 from `params.offset.unwrap_or(0)` to `params.offset.unwrap_or(0).min(MAX_OFFSET)`
  - [x] Return capped offset in response JSON (already the case via `offset` field)
  - [x] Add test: request with `offset=20000` returns response with `offset=10000`

- [x] Task 2: Unfiltered query warning and bounded default (AC: #2)
  - [x] In `list_tasks` handler, after parsing `queue` and `status`, detect both-None case
  - [x] Emit `warn!(event = "unfiltered_task_list", "GET /tasks called without queue or status filter")`
  - [x] When unfiltered AND no explicit limit: default to 100 (not DEFAULT_LIST_LIMIT=50). When unfiltered AND explicit limit provided: use the explicit limit, clamped to MAX_LIST_LIMIT=100 (same ceiling as filtered — only the *default* differs between branches)
  - [x] Implementation: `let limit = if queue.is_none() && status.is_none() { params.limit.unwrap_or(100).clamp(1, MAX_LIST_LIMIT) } else { params.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT) };`
  - [x] Update OpenAPI doc comment on `list_tasks` to note unfiltered behavior
  - [x] Add test: request with no filters and no limit returns at most 100 results (not 50)
  - [x] Add test: request with no filters and explicit `limit=10` returns at most 10 results
  - [x] Add test: request with no filters triggers warning log

- [x] Task 3: Pagination index migration (AC: #3)
  - [x] Create `migrations/0003_add_pagination_index.sql`
  - [x] SQL: `CREATE INDEX IF NOT EXISTS idx_tasks_pagination ON tasks (created_at, id);`
  - [x] Do NOT use `CONCURRENTLY` — SQLx runs migrations inside a transaction, which is incompatible with concurrent index creation. Pre-production, so plain CREATE INDEX is fine.
  - [x] Verify `EXPLAIN ANALYZE` uses the index for `ORDER BY created_at ASC, id ASC` queries

- [x] Task 4: Queue statistics active-only filter (AC: #4)
  - [x] Modify `queue_statistics()` SQL in `crates/infrastructure/src/adapters/postgres_task_repository.rs:662-670`
  - [x] Add `HAVING COUNT(*) FILTER (WHERE status IN ('pending', 'running')) > 0` clause
  - [x] Update existing queue_stats tests to verify terminal-only queues are excluded
  - [x] Add test: queue with only completed/failed tasks is absent from response

- [x] Task 5: Case-insensitive status parsing (AC: #5)
  - [x] Modify `parse_status_filter` in `crates/api/src/http/handlers/tasks.rs:266-277`
  - [x] Add `.to_ascii_lowercase()` on input `s` before the match block (same pattern as CLI `parse_status` in `crates/api/src/cli/tasks.rs:28`)
  - [x] Add tests: `PENDING`, `Pending`, `pEnDiNg` all resolve correctly

- [x] Task 6: CLI pagination bounds clamping (AC: #6)
  - [x] Modify `print_task_table` in `crates/api/src/cli/output.rs:88-92`
  - [x] Replace `offset + 1` with saturating arithmetic: `u64::from(offset).saturating_add(1).min(total)`
  - [x] Note: when `offset >= total` AND the API returns zero tasks, the early return at line 65-68 already shows "No tasks found." The fix is for the display math on the *last page* where `tasks` is non-empty but `offset + tasks.len()` may exceed `total` after concurrent deletions. Ensure the `{start}-{end}` range is clamped to `total`.
  - [x] Add CLI test exercising last-page edge case where `offset + tasks.len() > total`

- [x] Task 7: SQLx cache regeneration (AC: #7)
  - [x] Run `cargo sqlx prepare --workspace` after all migration and query changes
  - [x] Verify `cargo sqlx prepare --check --workspace` passes
  - [x] Commit updated `.sqlx/` cache files

- [x] Task 8: Full test pass and cleanup
  - [x] Run `cargo test --workspace` — all existing tests pass
  - [x] Run `cargo check-all` — clippy pedantic clean
  - [x] Run `cargo fmt --check` — formatting clean
  - [x] Docker container cleanup per CLAUDE.md

## Dev Notes

### Files to Modify

| File | Change |
|------|--------|
| `crates/api/src/http/handlers/tasks.rs` | AC1: offset cap, AC2: unfiltered warning, AC5: case-insensitive |
| `crates/infrastructure/src/adapters/postgres_task_repository.rs` | AC4: HAVING clause in queue_statistics |
| `crates/api/src/cli/output.rs` | AC6: saturating pagination bounds |
| `migrations/0003_add_pagination_index.sql` | AC3: new migration file |
| `.sqlx/*.json` | AC7: regenerated offline cache |

### Files to Add Tests To

| File | Tests |
|------|-------|
| `crates/api/tests/rest_api_test.rs` | offset capping, unfiltered warning, case-insensitive status, ghost queue exclusion |
| `crates/api/tests/cli_test.rs` | offset > total display |

### Architecture Compliance

- **Hexagonal layers**: All changes stay within existing layer boundaries. No new cross-layer dependencies.
- **Dynamic SQL**: `list_tasks` already uses dynamic SQL construction (not compile-time verified). The query pattern stays identical; only parameter processing changes.
- **Migrations**: Follow naming convention `{NNN}_{verb}_{noun}.sql`. Next number is `0003`.
- **Error handling**: Use existing `AppError::invalid_query_parameter()` for validation failures.
- **Instrumentation**: The `list_tasks` handler already has `#[instrument]`. The `warn!` for unfiltered queries follows the structured logging pattern established in Story 3.1.
- **Naming**: Constants use `SCREAMING_SNAKE_CASE` per architecture rules.
- **JSON fields**: `camelCase` via `serde(rename_all = "camelCase")` — no changes needed.

### Key Implementation Details

**AC1 — Offset capping**: The simplest approach is `.min(MAX_OFFSET)` after `unwrap_or(0)`. The capped value flows through to the response, so the caller sees the actual offset used. No error is returned — this is a soft cap, not a rejection.

**AC2 — Unfiltered query**: When no queue or status filter is provided, the default limit changes from 50 to 100. This is a separate code path from the filtered case — detect `queue.is_none() && status.is_none()` before limit computation. The warning is informational, not a rejection. The key is the structured `event = "unfiltered_task_list"` field for log filtering. Don't change the DEFAULT_LIST_LIMIT or MAX_LIST_LIMIT constants — add the unfiltered check as a separate condition before constructing `ListTasksFilter`.

**AC3 — Migration**: The `ORDER BY created_at ASC, id ASC` in `list_tasks` matches this index exactly. The existing `idx_tasks_claiming` index uses different columns (`queue, status, priority DESC, scheduled_at ASC`), so this new index covers a different access pattern. Since we're pre-production, `CONCURRENTLY` is not required — use a plain `CREATE INDEX`.

**AC4 — HAVING clause**: The current SQL groups by `queue` and counts by status filters. Adding `HAVING COUNT(*) FILTER (WHERE status IN ('pending', 'running')) > 0` filters out queues where all tasks are in terminal states (completed, failed, cancelled). This is a pure SQL change with no Rust code impact beyond tests. Alternative: a `WHERE status IN ('pending', 'running')` would skip terminal rows before aggregation (more efficient on large tables), but changes the COUNT semantics. The HAVING approach is preferred — it preserves the original query structure and is easier to reason about.

**AC5 — Case-insensitive**: The CLI already does `.to_ascii_lowercase()` at `crates/api/src/cli/tasks.rs:28`. Apply the identical pattern to the REST handler's `parse_status_filter`. Change the first line to `let s = s.to_ascii_lowercase();` and match on `s.as_str()`.

**AC6 — Saturating bounds**: The current `output.rs:90` line `offset + 1` can produce nonsensical output when `offset >= total` (e.g., "Showing 101-50 of 50 tasks"). Use `u64::from(offset)` for safe widening, then clamp all arithmetic. Note: AC1's offset cap (max 10,000) prevents u32 overflow in `offset + 1`, but the semantic issue (offset beyond total) still needs handling. When `tasks.is_empty()` AND `offset > 0`, the early return at line 65 already handles "No tasks found" but doesn't show pagination context. The fix should handle: (a) offset >= total → "Showing 0 of N tasks", (b) normal case with safe arithmetic.

**AC7 — SQLx**: The `queue_statistics()` query is compile-time verified (uses `sqlx::query_as` with a string literal). After modifying that SQL string, the `.sqlx/` cache must be regenerated. The `list_tasks` query uses dynamic SQL (`sqlx::query_as::<_, TaskRowWithTotal>(&sql)`) which is NOT in the offline cache.

### CRs Resolved by This Story

- **CR3**: Unbounded offset — cap at maximum
- **CR4**: Unfiltered GET /tasks — require filter or warning
- **CR5**: Pagination index — (created_at, id) composite
- **CR6**: queue_statistics() — filter to active queues
- **CR7**: parse_status_filter — case-insensitive
- **CR15**: output.rs pagination bounds — defensive check

### Deferred Work References

These items from `deferred-work.md` are directly resolved by this story:
- "Unbounded `offset` allows expensive full-table scans" (from Story 4.2 review)
- "Unfiltered `GET /tasks` triggers full-table COUNT" (from Story 4.2 review)
- "`queue_statistics()` includes historical queues with all-zero counts" (from Story 4.2 review)
- "No pagination index for `(created_at, id)`" (from Story 4.2 review)
- "`parse_status_filter` is case-sensitive" (from Story 4.2 review)

### Previous Story Intelligence

**From Epic 6 retrospective:**
- Clarity is the single biggest velocity lever. Precise story specs produce zero-debugging implementations.
- Security-adjacent stories need all review findings resolved before marking done.
- Deferred items must be assigned to target story at creation.

**From Story 6.10 (most recent):**
- 27 files modified across 4 crates; accessor migration was systematic.
- All `TaskRecord` fields now `pub(crate)` with typed accessor methods.
- Cross-epic dependency note: Story 7.2 changes `TaskRecord.payload` to `Arc<serde_json::Value>`. The accessor `pub fn payload(&self) -> &serde_json::Value` works transparently via `Arc::Deref`.

**From recent commits:**
- Builder pattern via `bon` established (Story 6.9).
- Structured error model with `PayloadErrorKind`/`ExecutionErrorKind` in place (Story 6.6).
- Database error scrubbing via `scrub_database_message` in `infrastructure/src/error.rs` (Story 6.7).
- Window function `COUNT(*) OVER()` already used in `list_tasks` (Story 6.8).

### Anti-Patterns to Avoid

- **Do NOT reject unfiltered queries** — AC2 says emit warning + bound, not reject with 422.
- **Do NOT change `DEFAULT_LIST_LIMIT` or `MAX_LIST_LIMIT` constants** — add separate unfiltered logic.
- **Do NOT add cursor-based pagination** — that's a future enhancement. This story hardens offset-based.
- **Do NOT modify the `ListTasksFilter` domain model** — capping happens at the API boundary, not domain.
- **Do NOT use `CONCURRENTLY` in migration** — SQLx runs migrations in transactions; `CONCURRENTLY` is incompatible.
- **Do NOT add new error types** — existing `AppError::invalid_query_parameter` and `warn!` logging suffice.

### Project Structure Notes

- All changes align with existing 4-crate hexagonal architecture.
- New migration file follows `migrations/{NNN}_{description}.sql` convention.
- Tests in `crates/api/tests/` follow existing patterns using shared test infrastructure.
- No new dependencies required.

### References

- [Source: docs/artifacts/planning/epics.md, Lines 549-591 — Story 7.1 definition and AC]
- [Source: docs/artifacts/planning/architecture.md — REST API patterns, database schema, testing standards]
- [Source: docs/artifacts/implementation/deferred-work.md, Lines 105-117 — Story 4.2 deferred items]
- [Source: docs/artifacts/implementation/epic-6-retro-2026-04-23.md — Retrospective lessons and team agreements]
- [Source: crates/api/src/http/handlers/tasks.rs:246-330 — Current list_tasks implementation]
- [Source: crates/infrastructure/src/adapters/postgres_task_repository.rs:564-696 — list_tasks + queue_statistics SQL]
- [Source: crates/api/src/cli/output.rs:50-93 — Current pagination display]
- [Source: crates/api/src/cli/tasks.rs:27-38 — CLI parse_status with to_ascii_lowercase pattern]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

N/A — no debugging needed; all changes compiled and tested on first pass.

### Completion Notes List

- AC1: Added `MAX_OFFSET = 10_000` constant; offset capped via `.min(MAX_OFFSET)` in list_tasks handler.
- AC2: Detect unfiltered queries (no queue/status), emit `warn!(event = "unfiltered_task_list")`, default limit to 100 (vs 50 for filtered). Updated OpenAPI doc comment.
- AC3: Created `migrations/0003_add_pagination_index.sql` with composite index on `(created_at, id)`.
- AC4: Added `HAVING COUNT(*) FILTER (WHERE status IN ('pending', 'running')) > 0` to queue_statistics SQL.
- AC5: Added `.to_ascii_lowercase()` to `parse_status_filter` — matches CLI pattern.
- AC6: Replaced `offset + 1` with saturating arithmetic; handles offset >= total edge case.
- AC7: Regenerated `.sqlx/` offline cache; `cargo sqlx prepare --check` passes.
- All workspace tests pass, clippy clean, fmt clean.

### File List

- `crates/api/src/http/handlers/tasks.rs` — AC1 offset cap, AC2 unfiltered warning, AC5 case-insensitive
- `crates/infrastructure/src/adapters/postgres_task_repository.rs` — AC4 HAVING clause
- `crates/api/src/cli/output.rs` — AC6 pagination bounds + unit tests
- `migrations/0003_add_pagination_index.sql` — AC3 new migration
- `.sqlx/query-*.json` — AC7 regenerated cache
- `crates/api/tests/rest_api_test.rs` — new integration tests (offset cap, unfiltered, case-insensitive, terminal queue exclusion)

### Change Log

- 2026-04-24: Story 7.1 implemented — all 7 ACs satisfied, 8 tasks completed.

### Review Findings

- [x] [Review][Decision] Offset Capping Feedback — Large offsets are silently capped at 10,000. Added \`warn!\` log. [AC1]
- [x] [Review][Patch] CLI Pagination Display Math — \`print_task_table\` range calculation fixed. [crates/api/src/cli/output.rs:88]
- [x] [Review][Patch] Status Filter Whitespace Sensitivity — \`parse_status_filter\` now trims input. [crates/api/src/http/handlers/tasks.rs:266]
- [x] [Review][Patch] Missing Instrumentation in Queue Stats — Added debug log. [crates/infrastructure/src/adapters/postgres_task_repository.rs:662]
