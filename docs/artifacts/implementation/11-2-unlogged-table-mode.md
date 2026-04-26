# Story 11.2: UNLOGGED Table Mode

Status: done

## Story

As an operator running high-throughput non-durable workloads,
I want to configure iron-defer to use UNLOGGED tables,
so that I get significantly higher write throughput at the cost of crash durability.

## Acceptance Criteria

1. **Given** `database.unlogged_tables: true` in configuration
   **When** the engine starts and runs migrations
   **Then** the tasks table is created as `UNLOGGED`

2. **Given** `unlogged_tables: true` AND `audit_log: true`
   **When** the engine starts
   **Then** startup is rejected with a clear error (mutual exclusion per FR40)

3. **Given** the configuration documentation and startup banner
   **When** UNLOGGED mode is enabled
   **Then** the documentation and logs explicitly warn that data is lost on Postgres crash

4. **Given** the benchmark suite on production-configured Postgres
   **When** UNLOGGED vs LOGGED throughput is compared (NFR-SC6)
   **Then** UNLOGGED delivers >= 5x throughput improvement
   **And** the benchmark runs on dedicated hardware, not in CI

## Functional Requirements Coverage

- **FR50:** Operator configures UNLOGGED tables via `database.unlogged_tables: true`
- **FR51:** Engine creates appropriate table type at startup based on configuration flag

## Tasks / Subtasks

- [x] Task 1: Runtime UNLOGGED table conversion (AC: 1)
  - [x] 1.1 No migration file needed — runtime post-migration step via `reconcile_table_persistence()`
  - [x] 1.2 `reconcile_table_persistence()` queries `pg_class.relpersistence` to detect current state
  - [x] 1.3 `ALTER TABLE tasks SET UNLOGGED` executed when config says UNLOGGED but table is LOGGED
  - [x] 1.4 `ALTER TABLE tasks SET LOGGED` executed when config says LOGGED but table is UNLOGGED
  - [x] 1.5 `task_audit_log` is never converted — FK constraint handling drops and re-adds as needed
  - [x] 1.6 Added `unlogged_tables: bool` field to `IronDefer` struct
  - [x] 1.7 Wired through `IronDeferBuilder::build()` from `DatabaseConfig.unlogged_tables`
  - [x] 1.8 Added `pub fn is_unlogged_tables(&self) -> bool` accessor

- [x] Task 2: Startup validation (AC: 2) — ALREADY IMPLEMENTED
  - [x] 2.1 Verified existing `DatabaseConfig::validate()` rejects both flags
  - [x] 2.2 Integration tests `unlogged_mutual_exclusion_rejects_startup` and `unlogged_mutual_exclusion_rejects_engine_build`
  - [x] 2.3 Validation runs in `build()` before migrations

- [x] Task 4: Startup logging and documentation (AC: 3)
  - [x] 4.1 `warn!` log emitted when `unlogged_tables: true`
  - [x] 4.2 `info!` log confirms persistence status: UNLOGGED or LOGGED
  - [x] 4.3 Added UNLOGGED mode section to `docs/guides/configuration.md`
  - [x] 4.4 Added UNLOGGED warning to README configuration section

- [x] Task 5: Integration tests (AC: 1-3)
  - [x] 5.1 `unlogged_mutual_exclusion_rejects_startup` — validates `DatabaseConfig::validate()` error
  - [x] 5.2 `unlogged_tables_flag_accepted` — validates config acceptance
  - [x] 5.3 `unlogged_tables_flag_converts_table` — queries `pg_class.relpersistence`, verifies UNLOGGED
  - [x] 5.4 `unlogged_mode_basic_operations` — full enqueue→claim→complete lifecycle in UNLOGGED mode
  - [x] 5.5 `unlogged_to_logged_restores_wal` — verifies restoration from UNLOGGED back to LOGGED
  - [x] 5.6 `unlogged_mutual_exclusion_rejects_engine_build` — end-to-end engine build rejection

- [x] Task 6: Benchmark scaffold (AC: 4) — informational, NOT CI-gated
  - [x] 6.1 Created `crates/api/benches/unlogged_throughput.rs` with Criterion
  - [x] 6.2 Single `DATABASE_URL` env var; engine toggles `unlogged_tables` config per batch
  - [x] 6.3 BATCH_SIZE=1000, measures enqueue + claim + complete throughput
  - [x] 6.4 Added `[[bench]]` entry to `crates/api/Cargo.toml` with `harness = false`
  - [x] 6.5 Documented hardware requirements in benchmark file header comment
  - [x] 6.6 NFR-SC6 target: >= 5x, logged with PASS/FAIL

## Dev Notes

### Architecture Compliance

**Hexagonal layer rules:**
- Configuration: `DatabaseConfig.unlogged_tables` already exists in `crates/application/src/config.rs` (line 30)
- Validation: Already implemented at `config.rs` lines 103-111
- Infrastructure: `PostgresTaskRepository` does NOT need changes — queries work identically on UNLOGGED tables
- API: `IronDefer` builder already accepts `DatabaseConfig` with `unlogged_tables` field

### Critical Implementation Details

1. **UNLOGGED tables and WAL:** PostgreSQL UNLOGGED tables skip write-ahead logging. This means: (a) inserts/updates are faster (~3-5x on tuned Postgres), (b) data is LOST on crash recovery (table is truncated to zero rows), (c) UNLOGGED tables are NOT replicated to standby servers.

2. **Migration strategy:** sqlx migrations are static SQL — can't conditionally use `CREATE UNLOGGED TABLE`. The architecture spec says "conditional migration — the migration runner checks the config flag." Implementation: run standard migrations (LOGGED table), then execute `ALTER TABLE tasks SET UNLOGGED` post-migration when the config flag is true. Viable because the project targets Postgres 16 (supports `ALTER TABLE SET UNLOGGED` since Postgres 15).

3. **Existing deployments switching modes:** `ALTER TABLE tasks SET UNLOGGED` acquires an `ACCESS EXCLUSIVE` lock but preserves all rows. No data loss. Document: "Switching persistence modes takes a brief exclusive lock. Schedule during low-traffic periods."

4. **`task_audit_log` is ALWAYS LOGGED:** Enforced by mutual exclusion (can't have both flags). Even if relaxed, audit log must be durable.

5. **Indexes survive UNLOGGED conversion:** `ALTER TABLE SET UNLOGGED` converts indexes automatically on Postgres 16.

### Source Tree Components to Touch

| File | Change |
|------|--------|
| `crates/api/src/lib.rs` | Add `unlogged_tables` field to `IronDefer` struct, post-migration `ALTER TABLE SET UNLOGGED` step, `is_unlogged_tables()` accessor, startup warning log |
| `crates/application/src/config.rs` | Verify existing validation (no changes expected) |
| `crates/api/tests/unlogged_test.rs` | **NEW** — integration tests |
| `crates/api/benches/unlogged_throughput.rs` | **NEW** — Criterion benchmark (informational) |
| `crates/api/Cargo.toml` | Add bench entry for `unlogged_throughput` |
| `docs/guides/configuration.md` | Update with UNLOGGED mode section (if file exists) |

### Testing Standards

- Integration tests in `crates/api/tests/unlogged_test.rs` as flat file
- Use `fresh_pool_on_shared_container()` — Postgres 16 supports `ALTER TABLE SET UNLOGGED`, so runtime conversion tests work directly
- **Caution:** `fresh_pool_on_shared_container()` shares a single database across tests. A test that converts the table to UNLOGGED will affect other tests in the same binary. Either: (a) use `fresh_unmigrated_pool()` for UNLOGGED tests to get an isolated container, or (b) restore the table to LOGGED in test teardown via `ALTER TABLE tasks SET LOGGED`
- Test the validation/startup path thoroughly — the actual UNLOGGED table performance is verified by the benchmark on dedicated hardware, not in tests
- Skip gracefully when Docker is unavailable

### Critical Constraints

1. **NFR-SC6 cannot be verified in CI.** The >= 5x throughput improvement requires production-configured Postgres on dedicated hardware. Testcontainers with default config won't show representative WAL overhead. The benchmark scaffold is provided for manual/scheduled runs.

2. **Postgres 16 is the target.** Testcontainers uses Postgres 16 (default from `testcontainers-modules`). `ALTER TABLE SET UNLOGGED` is fully supported (Postgres 15+). Use it directly — no table recreate approach needed.

3. **Mutual exclusion validation already works.** `DatabaseConfig::validate()` at `config.rs:103-111` already rejects `unlogged_tables + audit_log`. Just verify this with a test.

4. **This story is about the MECHANISM, not the benchmark result.** The mechanism must be implemented and tested. The throughput benchmark is informational scaffolding — the actual NFR-SC6 validation is in Story 11.3.

5. **No data migration needed for new column.** Unlike Story 11.1 (checkpoint), this story doesn't add columns — it changes table persistence characteristics.

### Previous Story Intelligence

**From Story 10.2 (audit log — mutual exclusion context):**
- `DatabaseConfig.audit_log` and `DatabaseConfig.unlogged_tables` already coexist in the config struct
- Validation at config.rs:103-111 covers the mutual exclusion
- Audit log uses `task_audit_log` table — completely independent of tasks table persistence

**From Story 10.3 (compliance E2E tests — benchmark patterns):**
- `audit_overhead.rs` benchmark provides the pattern: Criterion 0.5, custom `iter_custom()`, `DATABASE_URL` env var
- Benchmark results are informational, not CI-gated — same approach for UNLOGGED benchmark

**From Epic 7 (Story 7.4 — Kubernetes/Docker):**
- Configuration is loaded via figment with env overlay: `IRON_DEFER__DATABASE__UNLOGGED_TABLES=true`
- Docker/K8s manifests may need an env var example for UNLOGGED mode

### Existing Infrastructure to Reuse

- `DatabaseConfig` with `unlogged_tables: bool` — already exists, defaults to `false`
- `DatabaseConfig::validate()` — mutual exclusion already enforced
- `IronDeferBuilder.database_config()` — already accepts the full config including the flag
- Criterion 0.5 benchmark infrastructure from `crates/api/benches/`

### References

- [Source: docs/artifacts/planning/epics.md — Epic 11, Story 11.2 (lines 1181-1209)]
- [Source: docs/artifacts/planning/prd.md — FR50, FR51 (lines 967-968)]
- [Source: docs/artifacts/planning/prd.md — G3 spec (lines 175-181)]
- [Source: docs/artifacts/planning/prd.md — NFR-SC6 (line 1066)]
- [Source: docs/artifacts/planning/architecture.md — Conditional Table Creation G3 (lines 1823-1825)]
- [Source: docs/artifacts/planning/architecture.md — Cross-Feature Matrix G3+G5 (line 2035)]
- [Source: crates/application/src/config.rs — DatabaseConfig.unlogged_tables (line 30), validate() (lines 103-111)]
- [Source: crates/api/src/lib.rs — IronDeferBuilder (lines 1073-1287)]
- [Source: migrations/0001_create_tasks_table.sql — CREATE TABLE tasks DDL]
- [Source: crates/api/benches/audit_overhead.rs — Criterion benchmark pattern]

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List
- Implemented post-migration `reconcile_table_persistence()` in `IronDefer::build()` — handles LOGGED↔UNLOGGED conversion with FK constraint management for `task_audit_log`
- FK constraint from `task_audit_log.task_id → tasks.id` must be dropped before ALTER TABLE SET UNLOGGED (PostgreSQL rejects LOGGED→UNLOGGED FK references); restored on SET LOGGED
- Added `is_unlogged_tables()` accessor to `IronDefer`
- Fixed pre-existing compilation issues: `tasks.rs:382` used non-existent `find_by_id` (→ `find`), `audit_log_test.rs` compared `TaskStatus` with strings, `audit_overhead.rs` benchmark used non-existent `wait_for_status` (→ poll loop)
- Regenerated `.sqlx/` offline cache after code review changed checkpoint query to include `worker_id`/`claimed_by` guard
- 6 integration tests (serialized via `serial_test` to avoid concurrent table persistence mutations)
- Criterion benchmark scaffold with BATCH_SIZE=1000 and NFR-SC6 5x target reporting

### File List
- crates/api/src/lib.rs — `unlogged_tables` field, `is_unlogged_tables()`, `reconcile_table_persistence()`, `set_table_persistence()`
- crates/api/src/http/handlers/tasks.rs — fixed `find_by_id` → `find`, `task_id` → `id` in NotFound
- crates/api/tests/unlogged_test.rs — **NEW** 6 integration tests
- crates/api/tests/audit_log_test.rs — fixed TaskStatus comparisons
- crates/api/benches/audit_overhead.rs — replaced `wait_for_status` with poll loop
- crates/api/benches/unlogged_throughput.rs — **NEW** Criterion benchmark
- crates/api/Cargo.toml — added `[[bench]]` entry for `unlogged_throughput`
- docs/guides/configuration.md — added UNLOGGED mode section
- README.md — added UNLOGGED mode warning in Configuration section
- .sqlx/ — regenerated offline cache
- docs/artifacts/implementation/sprint-status.yaml — story status updates
- docs/artifacts/implementation/11-2-unlogged-table-mode.md — task checkboxes updated

### Review Findings (2026-04-25)

- [x] [Review][Decision] Runtime DDL (ALTER TABLE) Security Risk — RESOLVED: Kept as is to adhere to spec. Requirement for elevated DB permissions and ACCESS EXCLUSIVE lock risk accepted.
- [x] [Review][Patch] Restoration Failure due to Orphaned Audit Logs — RESOLVED: Added logic to clear orphaned \`task_audit_log\` rows before restoring FK constraint in \`set_table_persistence\`.
- [x] [Review][Patch] Busy-wait Polling in Benchmarks — RESOLVED: Confirmed 5ms loop is the intended performance mechanism for this codebase (no event notification system available).
- [x] [Review][Patch] Checkpoint Row-Affected Validation — RESOLVED: Verified that \`PostgresCheckpointWriter\` already performs row-affected validation and returns \`NotInExpectedState\` if the task is not running/owned by the worker. (Note: Initial patch suggestion incorrectly stated this was missing; re-verification confirmed it is present in \`infrastructure\` layer).
