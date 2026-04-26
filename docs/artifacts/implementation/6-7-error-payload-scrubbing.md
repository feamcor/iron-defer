# Story 6.7: Error Payload Scrubbing

Status: review

## Story

As a developer,
I want database error messages scrubbed of potential payload content,
so that task payloads never leak through error channels under the default `log_payload = false` configuration.

## Acceptance Criteria

1. **`sqlx::Error::Database` scrubbing (CR14)**

   **Given** any `sqlx::Error::Database` error that propagates through the adapter layer
   **When** it reaches the `PostgresAdapterError::from(sqlx::Error)` conversion
   **Then** the error message is scrubbed for potential payload content — extending the existing `scrub_url` pattern to cover `sqlx::Error::Database` details (not just `sqlx::Error::Configuration`)
   **And** the scrubbing applies a conservative approach: strip any substring that looks like JSON or contains common PII patterns

2. **Default privacy enforcement**

   **Given** the default configuration (`log_payload = false`)
   **When** a database error containing task payload content is logged via `#[instrument(err)]`
   **Then** no task payload content appears in the structured log output

3. **Combined verification**

   **Given** the scrubbing changes
   **When** `cargo test --workspace` runs
   **Then** the existing `payload_privacy_*` tests continue to pass
   **And** a new test verifies that `sqlx::Error::Database` messages are scrubbed before reaching log output

## Tasks / Subtasks

- [x] **Task 1: Implement `scrub_database_message` function** (AC: 1)
  - [x] 1.1: Create a `scrub_database_message(msg: &str) -> String` function in `crates/infrastructure/src/error.rs` (alongside the existing `scrub_message`)
  - [x] 1.2: The function must strip JSON-like substrings: detect `{...}` and `[...]` blocks and replace with `<scrubbed-json>`
  - [x] 1.3: Do NOT blindly strip all quoted strings — Postgres uses double-quotes for identifiers (`"tasks"`, `"payload_check"`) which are safe metadata. Only apply quoted-value scrubbing in `scrub_detail` (the DETAIL line with `Failing row contains (...)`) where values are known to contain row data. The primary error `message()` should only have JSON blocks scrubbed, not quoted identifiers.
  - [x] 1.4: Preserve Postgres diagnostic context (SQLSTATE code, constraint name, table name) while stripping values — these appear as double-quoted identifiers in the `message()` field and must NOT be scrubbed
  - [x] 1.5: Handle nested JSON (balanced brace counting)

- [x] **Task 2: Extend `PostgresAdapterError::from(sqlx::Error)` to handle Database variant** (AC: 1)
  - [x] 2.1: In `crates/infrastructure/src/error.rs:42–66`, add a branch for `sqlx::Error::Database` before the `Query` fallthrough
  - [x] 2.2: Extract the error message via `db_err.message()` and apply `scrub_database_message`
  - [x] 2.3: Map to a new `PostgresAdapterError` variant or reuse `Configuration` with the scrubbed message — recommend a new `DatabaseScrubbed { message: String, code: Option<String> }` variant that preserves the SQLSTATE code for programmatic use while scrubbing the human-readable message
  - [x] 2.4: Ensure the `From<PostgresAdapterError> for TaskError` impl (lines 108–118) continues to collapse all variants into `TaskError::Storage`

- [x] **Task 3: Update `is_pool_timeout` classifier for new variant** (AC: 1)
  - [x] 3.0: In `crates/infrastructure/src/db.rs:132–151`, add a `downcast_ref::<PostgresAdapterError>()` check that matches `DatabaseScrubbed { code: Some(c), .. }` where `c.starts_with("08")` — preserves class-08 connection-exception detection that was previously reached via `sqlx::Error::Database` downcast
  - [x] 3.0b: Verify existing `task_error_storage_preserves_sqlx_pool_timeout_source` test (error.rs:298) still passes — it tests `PoolTimedOut` which is unaffected
  - [x] 3.0c: Add a test verifying class-08 `DatabaseScrubbed` errors are correctly classified as pool saturation

- [x] **Task 4: Add unit tests for database message scrubbing** (AC: 3)
  - [x] 4.1: Test: Postgres constraint violation message containing JSON payload is scrubbed
  - [x] 4.2: Test: Postgres CHECK constraint message containing quoted values is scrubbed
  - [x] 4.3: Test: Plain SQL error message without payload content passes through unchanged (no false-positive scrubbing)
  - [x] 4.4: Test: SQLSTATE code is preserved in the scrubbed output
  - [x] 4.5: Test: Nested JSON in error message is fully scrubbed

- [x] **Task 5: Verify existing payload_privacy tests pass** (AC: 3)
  - [x] 5.1: `cargo test -p iron-defer -- payload_privacy` — all 5 tests pass
  - [x] 5.2: `cargo test -p iron-defer-infrastructure -- scrub` — all scrub tests pass

- [x] **Task 6: Verify no regressions** (AC: 3)
  - [x] 6.1: `cargo test --workspace` — all tests pass (259 tests)
  - [x] 6.2: `cargo clippy --workspace --all-targets -- -D clippy::pedantic` — no errors (fixed 3 pre-existing issues in application/api crates)
  - [x] 6.3: `cargo fmt --check` — clean

## Dev Notes

### Architecture Compliance

- **NFR-S2 (security lines 800–801):** Task payload content must not appear in log output, OTel traces, or emitted metrics by default.
- **D4.3 (architecture lines 417–421):** Payload not logged by default (`log_payload: false`). Spans: `skip(payload)` on all instrumented methods unless `log_payload = true`.
- **Error conversion (architecture lines 702–710):** Never discard error context. The scrubbed variant must still preserve diagnostic information (SQLSTATE code, constraint name) while stripping values.
- **Existing scrub pattern:** `scrub_url` in `crates/infrastructure/src/observability/tracing.rs:116–175` and `scrub_message` in `crates/infrastructure/src/error.rs:68–98` establish the pattern. This story extends it to a new error class.

### Critical Implementation Guidance

**Current gap:**

The `PostgresAdapterError::from(sqlx::Error)` impl at `crates/infrastructure/src/error.rs:42–66` handles only `sqlx::Error::Configuration`:

```rust
impl From<sqlx::Error> for PostgresAdapterError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Configuration(ref inner) = err {
            return Self::Configuration {
                message: scrub_message(&inner.to_string()),
            };
        }
        Self::Query { source: err }   // ← Database falls through here UNSCRUBBED
    }
}
```

`sqlx::Error::Database` variants pass through to `Query { source: err }`, which preserves the full error chain including any payload content in constraint violation messages.

**Risk assessment:**

Currently there are NO CHECK or UNIQUE constraints on the `payload` column (verified in `migrations/0001_create_tasks_table.sql` and `0002_add_claim_check.sql`). The risk is **hypothetical but real** — future migrations could add constraints, and Postgres constraint violation messages include the rejected value verbatim:

```
ERROR: new row for relation "tasks" violates check constraint "tasks_payload_size_check"
DETAIL: Failing row contains (..., {"ssn":"123-45-6789","credit_card":"4111..."}, ...).
```

The `DETAIL` line would contain the full payload, and it flows through `sqlx::Error::Database::message()` and `sqlx::Error::Database::detail()`.

**Recommended `scrub_database_message` implementation:**

```rust
/// Scrub potential payload content from a Postgres database error message.
///
/// Applies conservative scrubbing:
/// 1. JSON blocks (`{...}`, `[...]`) → `<scrubbed-json>`
/// 2. Quoted string values in DETAIL lines → `<scrubbed>`
/// 3. Preserves SQLSTATE codes, constraint names, and table names
fn scrub_database_message(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut chars = msg.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => {
                // Skip balanced JSON block
                let close = if c == '{' { '}' } else { ']' };
                let mut depth = 1u32;
                while let Some(inner) = chars.next() {
                    if inner == c {
                        depth += 1;
                    } else if inner == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                out.push_str("<scrubbed-json>");
            }
            _ => out.push(c),
        }
    }
    out
}
```

**For the DETAIL line scrubbing** (which contains the full row as a tuple), a separate function or regex can strip the content between `contains (` and the closing `)`:

```rust
/// Scrub the DETAIL portion of a Postgres constraint violation.
///
/// Postgres format: "Failing row contains (col1, col2, ..., colN)."
/// Replace the content between parentheses with `<scrubbed>`.
fn scrub_detail(detail: &str) -> String {
    if let Some(start) = detail.find("contains (") {
        let prefix = &detail[..start + "contains (".len()];
        if let Some(end) = detail[start..].rfind(')') {
            let suffix = &detail[start + end..];
            return format!("{prefix}<scrubbed>{suffix}");
        }
    }
    detail.to_owned()
}
```

**Updated `From` impl:**

```rust
impl From<sqlx::Error> for PostgresAdapterError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Configuration(ref inner) = err {
            return Self::Configuration {
                message: scrub_message(&inner.to_string()),
            };
        }
        if let sqlx::Error::Database(ref db_err) = err {
            let scrubbed_msg = scrub_database_message(db_err.message());
            let scrubbed_detail = db_err.detail().map(|d| scrub_detail(d));
            let code = db_err.code().map(|c| c.to_string());
            let mut full_msg = scrubbed_msg;
            if let Some(detail) = scrubbed_detail {
                full_msg = format!("{full_msg} DETAIL: {detail}");
            }
            return Self::DatabaseScrubbed {
                message: full_msg,
                code,
            };
        }
        Self::Query { source: err }
    }
}
```

**New `PostgresAdapterError` variant:**

Add to the enum at `error.rs:30–40`:

```rust
#[derive(Debug, Error)]
pub(crate) enum PostgresAdapterError {
    #[error("database query failed: {source}")]
    Query { source: sqlx::Error },

    #[error("database configuration error: {message}")]
    Configuration { message: String },

    #[error("database error (scrubbed): {message}")]
    DatabaseScrubbed {
        message: String,
        code: Option<String>,
    },

    #[error("row mapping failed: {reason}")]
    Mapping { reason: String },
}
```

The new `DatabaseScrubbed` variant:
- Preserves the SQLSTATE `code` (e.g., `"23514"` for CHECK violation) for programmatic use
- Scrubs the human-readable `message` and `detail` to remove potential payload content
- The `From<PostgresAdapterError> for TaskError` impl (lines 108–118) already handles all variants via `TaskError::Storage { source: Box::new(err) }` — no change needed there

**Why a new variant instead of reusing `Configuration`:**

`Configuration` is semantically wrong for database errors — it represents connection-string/config issues. A new variant keeps the semantic distinction clear so any future code that matches on `PostgresAdapterError` variants can distinguish between config issues and constraint violations.

**`sqlx::Error::Database` API reference:**

The `sqlx::error::DatabaseError` trait provides:
- `message() -> &str` — primary error message (always present)
- `detail() -> Option<&str>` — DETAIL line (contains rejected row data on constraint violations)
- `hint() -> Option<&str>` — HINT line (safe, no payload)
- `code() -> Option<Cow<str>>` — SQLSTATE code (safe, no payload)
- `constraint() -> Option<&str>` — constraint name (safe, no payload)
- `table() -> Option<&str>` — table name (safe, no payload)

Only `message()` and `detail()` can contain payload content. `code()`, `constraint()`, and `table()` are safe metadata.

### Previous Story Intelligence

**From Story 6.6 (ready-for-dev):**
- Error model restructured: `TaskError::InvalidPayload` and `ExecutionFailed` now use structured source types. `TaskError::Storage` variant is unchanged — it still carries `Box<dyn Error>`. The scrubbing here applies at the `PostgresAdapterError` level (before boxing into `Storage`), so no interaction with 6.6 changes.
- New `TaskError::Migration` variant added — does not affect scrubbing since migration errors are a separate variant.

**From Story 6.2 (done):**
- Clippy pedantic enforced workspace-wide.

**From Story 3.1 (done):**
- `scrub_url` and `scrub_message` established the scrubbing pattern.
- Payload privacy tests established with `#[tracing_test::traced_test]`.

### Git Intelligence

Last code commit: `7ed6fc8`. Error scrubbing code last modified in Story 3.1 (URL scrubbing).

### Key Types and Locations (verified current)

| Type/Function | Location | Relevance |
|---|---|---|
| `PostgresAdapterError` enum | `crates/infrastructure/src/error.rs:30–40` | AC 1 — add `DatabaseScrubbed` variant |
| `From<sqlx::Error> for PostgresAdapterError` | `crates/infrastructure/src/error.rs:42–66` | AC 1 — add Database branch |
| `From<PostgresAdapterError> for TaskError` | `crates/infrastructure/src/error.rs:108–118` | AC 1 — verify unchanged |
| `scrub_message` | `crates/infrastructure/src/error.rs:68–98` | Context — existing URL scrub pattern |
| `scrub_url` | `crates/infrastructure/src/observability/tracing.rs:116–175` | Context — existing URL scrub pattern |
| `#[instrument(err)]` methods | `crates/infrastructure/src/adapters/postgres_task_repository.rs` | AC 2 — where scrubbed errors are logged |
| `log_payload` config | `crates/application/src/config.rs:43` | Context — payload privacy control |
| `payload_privacy_*` tests | `crates/application/src/services/worker.rs:1413–1749` | AC 3 — must continue passing |
| `payload_privacy_*` test | `crates/api/tests/observability_test.rs:51` | AC 3 — must continue passing |
| `migrations/0001_*.sql` | `migrations/0001_create_tasks_table.sql` | Context — no payload constraints currently |
| `is_pool_timeout` classifier | `crates/infrastructure/src/db.rs:132–151` | Context — walks source chain; Database variant needs different handling |

### `is_pool_timeout` interaction — MUST UPDATE CLASSIFIER

The `is_pool_timeout` classifier at `db.rs:132–151` walks the error source chain via `downcast_ref::<sqlx::Error>()`. It currently matches `sqlx::Error::Database(db_err)` where SQLSTATE class is `08` (connection exceptions like "server closed the connection") — line 141.

After this story, ALL `sqlx::Error::Database` variants are intercepted in the `From` impl and become `DatabaseScrubbed { message, code }`. The original `sqlx::Error::Database` is NO LONGER in the error chain — `is_pool_timeout` cannot downcast to it.

**This breaks class-08 connection-exception detection.** The fix:

1. Add a `downcast_ref::<PostgresAdapterError>()` check to `is_pool_timeout` (in addition to the existing `sqlx::Error` check)
2. When found, check if it's `DatabaseScrubbed { code: Some(c), .. }` where `c.starts_with("08")`
3. This preserves the existing behavior: class-08 database errors are still classified as saturation events

```rust
// Add to is_pool_timeout, BEFORE the sqlx::Error downcast:
if let Some(adapter_err) = current.downcast_ref::<PostgresAdapterError>() {
    return match adapter_err {
        PostgresAdapterError::DatabaseScrubbed { code: Some(c), .. } => c.starts_with("08"),
        _ => false,
    };
}
```

Note: `PostgresAdapterError` is `pub(crate)` in the infrastructure crate, so `is_pool_timeout` (also in the infrastructure crate) CAN access it. No visibility change needed.

**Non-class-08 database errors** (constraint violations, syntax errors, etc.) correctly return `false` from the classifier — they are NOT pool-saturation events. Only class-08 (connection exceptions) should trigger the saturation path.

- `sqlx::Error::PoolTimedOut`, `PoolClosed`, `Io`, `WorkerCrashed` — still fall through to `Query { source }` and are matched by the existing sqlx downcast. Unaffected.

### Dependencies

No new crate dependencies. The scrubbing uses string manipulation only.

### Project Structure Notes

- **Modified files only:**
  - `crates/infrastructure/src/error.rs` — new variant, new scrub functions, new tests
- **No new files created**
- **No schema changes, no `.sqlx/` regeneration needed**

### Out of Scope

- **State transition validation** (`scheduled_at` range check) — Story 6.8 (CR9)
- **Builder pattern / field visibility changes** — Stories 6.9, 6.10
- **Full PII scrubbing framework** (regex-based email/SSN detection) — Growth phase; this story applies conservative structural scrubbing (JSON blocks, quoted values, DETAIL lines) not content-aware PII detection
- **`sqlx::Error::Io` or other variants** — only `Database` and `Configuration` can carry user data; other variants contain system-level information only

### References

- [Source: `docs/artifacts/planning/epics.md` lines 427–446] — Story 6.7 acceptance criteria
- [Source: `docs/artifacts/planning/architecture.md` lines 417–421] — D4.3 Payload privacy
- [Source: `docs/artifacts/implementation/deferred-work.md` line 57] — CR14: sqlx::Error::Database payload leakage
- [Source: `crates/infrastructure/src/error.rs:42–66`] — Current From<sqlx::Error> impl (no Database handling)
- [Source: `crates/infrastructure/src/error.rs:30–40`] — PostgresAdapterError enum
- [Source: `crates/infrastructure/src/observability/tracing.rs:116–175`] — scrub_url implementation
- [Source: `crates/infrastructure/src/error.rs:68–98`] — scrub_message implementation
- [Source: `migrations/0001_create_tasks_table.sql`] — No payload constraints currently
- [Source: `crates/infrastructure/src/db.rs:132–151`] — is_pool_timeout classifier

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- Initial `is_pool_timeout` implementation matched all `PostgresAdapterError` variants, causing `Query`-wrapped `PoolTimedOut`/`PoolClosed`/`Io`/`WorkerCrashed` to short-circuit to false. Fixed by only intercepting `DatabaseScrubbed` variant specifically.
- `sqlx::error::DatabaseError::detail()` is not on the generic trait — it's on `PgDatabaseError`. Used `try_downcast_ref::<sqlx::postgres::PgDatabaseError>()` to access DETAIL line.

### Completion Notes List

- Implemented `scrub_database_message` function with balanced brace counting for JSON blocks (`{...}` and `[...]`), replacing with `<scrubbed-json>` placeholder
- Implemented `scrub_detail` function to strip row content from Postgres DETAIL lines (`Failing row contains (...)`)
- Added `DatabaseScrubbed { message, code }` variant to `PostgresAdapterError` — preserves SQLSTATE code for programmatic use while scrubbing human-readable content
- Updated `From<sqlx::Error>` to intercept `Database` variant, scrub message + DETAIL, and map to `DatabaseScrubbed`
- Updated `is_pool_timeout` classifier in `db.rs` to detect class-08 connection exceptions on `DatabaseScrubbed` variant
- Added 10 new tests: 7 unit tests for scrubbing functions + variant behavior, 3 tests for `is_pool_timeout` with `DatabaseScrubbed`
- Fixed 3 pre-existing clippy pedantic issues in application/api crates (missing `# Panics` doc, missing `# Errors` doc, uninlined format args)
- All 259 workspace tests pass, clippy pedantic clean, fmt clean

### Change Log

- 2026-04-23: Implemented error payload scrubbing (Story 6.7) — `scrub_database_message`, `scrub_detail`, `DatabaseScrubbed` variant, `is_pool_timeout` classifier update, 10 new tests, 3 pre-existing clippy fixes

### File List

- crates/infrastructure/src/error.rs (modified: new `DatabaseScrubbed` variant, `scrub_database_message`, `scrub_detail`, updated `From<sqlx::Error>` impl, 7 new tests)
- crates/infrastructure/src/db.rs (modified: `is_pool_timeout` now detects `DatabaseScrubbed` class-08, 3 new tests)
- crates/application/src/services/worker.rs (modified: added `# Panics` doc to `run_poll_loop` — pre-existing clippy fix)
- crates/api/src/lib.rs (modified: added `# Errors` doc to `inspect` — pre-existing clippy fix)
- crates/api/src/http/errors.rs (modified: inlined format args — pre-existing clippy fix)

### Review Findings

- [ ] [Review][Patch] Sensitive Data Leakage (Unique Constraints) [crates/infrastructure/src/error.rs:161]
- [ ] [Review][Patch] Naive JSON Parsing (String-Unaware) [crates/infrastructure/src/error.rs:134]
- [ ] [Review][Patch] Inconsistent URL Scrubbing in DB Errors [crates/infrastructure/src/error.rs:74]
- [ ] [Review][Patch] Missing Log-Level Verification Test [crates/infrastructure/src/error.rs:409]
- [ ] [Review][Patch] Fragile Detail Scrubbing & Redundant Headers [crates/infrastructure/src/error.rs:83]
- [ ] [Review][Patch] Outdated TaskError Conversion Comments [crates/infrastructure/src/error.rs:169]
- [x] [Review][Defer] Unhandled Panic in run_poll_loop [crates/application/src/services/worker.rs:146] — deferred, pre-existing
- [x] [Review][Defer] Hardcoded Error Depth Limit [crates/infrastructure/src/db.rs:133] — deferred, pre-existing
