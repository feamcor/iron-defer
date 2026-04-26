# ADR-0002: Error Handling Strategy

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

Error handling in Rust is explicit and powerful, but inconsistent use of error crates creates friction: untyped errors leak across boundaries, `unwrap()` causes unexpected panics in production, and error context is lost across async boundaries.

We need a consistent, layered strategy that:
- Keeps errors typed and matchable in domain and application code
- Provides rich human-readable context at the binary boundary
- Prevents panics in library code
- Integrates with structured logging via `tracing`

## Decision

We adopt a **layered error model** with different crates at different architectural levels.

## Error Crate Assignments

| Layer | Crate | Rationale |
|-------|-------|-----------|
| `domain` | `thiserror` | Typed, matchable, no runtime cost |
| `application` | `thiserror` | Typed orchestration errors, wraps domain errors |
| `infrastructure` | `thiserror` | Typed adapter errors, wraps external crate errors |
| binary (`api/main`) | `color-eyre` | Rich terminal output, tracing span integration |

**`anyhow`** is permitted only in test code or one-off CLI utilities. Never in library crates.

## Domain Errors

```rust
// crates/domain/src/errors.rs
use thiserror::Error;
use crate::model::{TaskId, WorkerId};

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("task not found: {id}")]
    NotFound { id: TaskId },

    #[error("task {id} is already claimed by worker {worker_id}")]
    AlreadyClaimed { id: TaskId, worker_id: WorkerId },

    #[error("task payload is invalid: {reason}")]
    InvalidPayload { reason: String },

    #[error("task execution failed: {source}")]
    ExecutionFailed {
        #[from]
        source: ExecutionError,
    },
}
```

Rules:
- One error enum per domain concept (not one global enum)
- Use `#[from]` sparingly — only when the conversion is always valid and unambiguous
- Include enough context in the error message to diagnose without a stack trace
- Derive `Debug` always; derive `Clone` and `PartialEq` where meaningful for tests

## Application Errors

```rust
// crates/application/src/errors.rs
#[derive(Debug, Error)]
pub enum ScheduleTaskError {
    #[error("task with id {id} already exists")]
    Duplicate { id: TaskId },

    #[error("repository error: {source}")]
    Repository {
        #[from]
        source: TaskError,
    },

    #[error("queue unavailable: {source}")]
    QueueUnavailable { source: Box<dyn std::error::Error + Send + Sync> },
}
```

## Infrastructure Errors

Infrastructure errors wrap external crate errors and translate them to types the application understands:

```rust
// crates/infrastructure/src/errors.rs
#[derive(Debug, Error)]
pub enum PostgresAdapterError {
    #[error("database query failed: {source}")]
    Query {
        #[from]
        source: sqlx::Error,
    },

    #[error("row mapping failed: {reason}")]
    Mapping { reason: String },
}

// Translate to domain error in the adapter impl:
impl From<PostgresAdapterError> for TaskError {
    fn from(e: PostgresAdapterError) -> Self {
        match e {
            PostgresAdapterError::Query { source } if is_not_found(&source) => {
                TaskError::NotFound { id: /* extract */ }
            }
            _ => TaskError::ExecutionFailed { source: e.into() },
        }
    }
}
```

## Binary Boundary

```rust
// crates/api/src/main.rs
use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;  // sets up pretty error rendering + backtraces

    // All ? operators from here produce rich, contextualized output on failure
    let config = load_config(&cli)?;
    let app = build_app(config).await?;
    app.run().await?;

    Ok(())
}
```

For adding context to errors at the boundary:

```rust
use color_eyre::eyre::WrapErr;

let pool = PgPool::connect(&url)
    .await
    .wrap_err("failed to connect to PostgreSQL")?;
```

## Panic Policy

| Context | Policy |
|---------|--------|
| Library crate (`domain`, `application`, `infrastructure`) | **Never panic**. Use `Result`. |
| Binary entry point | `panic!` only for unrecoverable startup failures (misconfiguration) |
| `unwrap()` | Forbidden in library code |
| `expect("msg")` | Permitted only when the None/Err case is a logic bug, not a runtime condition. Message must explain the invariant. |
| `unreachable!()` | Permitted with a comment explaining why it is truly unreachable |

```rust
// Acceptable: documents a real invariant
let id = parsed_uuid.expect("UUID was validated at deserialization boundary");

// Not acceptable: lazy error suppression
let result = operation().unwrap();
```

## Error and Tracing Integration

Always attach span context when propagating errors across async boundaries:

```rust
use tracing::instrument;

#[instrument(err, fields(task_id = %id))]
async fn execute_task(&self, id: &TaskId) -> Result<(), TaskError> {
    // errors are automatically recorded in the span
}
```

Use `#[instrument(err)]` on all service and repository methods. This ensures errors are visible in structured logs without extra boilerplate.

## Consequences

**Positive:**
- Errors are typed and matchable at every layer
- No surprise panics in library code
- Rich diagnostics in binary output via `color-eyre`
- Error context preserved across async boundaries via `tracing`

**Negative:**
- More error type definitions than a single `anyhow`-everywhere approach
- `From` impls between layers require maintenance — accepted for the type safety gained

## References

- [`thiserror` crate](https://docs.rs/thiserror)
- [`color-eyre` crate](https://docs.rs/color-eyre)
- [Rust Error Handling — The Book](https://doc.rust-lang.org/book/ch09-00-error-handling.html)
- [Microsoft Rust Guidelines — Error Handling](https://microsoft.github.io/rust-guidelines/)
