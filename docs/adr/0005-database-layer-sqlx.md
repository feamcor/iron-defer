# ADR-0005: Database Layer with SQLx

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

iron-defer requires durable, reliable storage for task state. The storage layer must:
- Provide compile-time SQL verification to catch query errors before runtime
- Support async access compatible with the Tokio runtime
- Enforce the hexagonal architecture boundary — DB types must not leak into the domain
- Support schema migrations as first-class versioned artifacts
- Enable realistic integration testing against real PostgreSQL

## Decision

We use **SQLx** with PostgreSQL as the primary database. Migrations are managed by `sqlx-cli` and embedded at runtime via `sqlx::migrate!`.

## Dependency Configuration

```toml
# crates/infrastructure/Cargo.toml
[dependencies]
sqlx = { version = "0.8", features = [
    "runtime-tokio-rustls",  # tokio runtime + rustls TLS (no OpenSSL dependency)
    "postgres",
    "uuid",
    "chrono",
    "json",
    "migrate",
] }
```

Use `runtime-tokio-rustls` — avoids the OpenSSL dependency for simpler cross-compilation and supply chain reduction.

## Connection Pool

```rust
// crates/infrastructure/src/db.rs
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub async fn create_pool(config: &DatabaseConfig) -> color_eyre::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(Duration::from_secs(config.connect_timeout_secs))
        .connect(&config.url)
        .await
        .wrap_err("failed to create database connection pool")?;

    Ok(pool)
}
```

- `PgPool` is `Clone` (cheap Arc clone) — pass to adapters via application state
- Never construct a pool per-request or per-query
- Set `max_connections` from config — never use defaults blindly

## Migrations

Migrations live in `migrations/` at workspace root and are managed by `sqlx-cli`:

```bash
# Create a new migration
sqlx migrate add create_tasks_table

# Run pending migrations
sqlx migrate run

# Revert last migration
sqlx migrate revert
```

Migrations are embedded and run automatically at startup:

```rust
// crates/api/src/main.rs
sqlx::migrate!("../../migrations")
    .run(&pool)
    .await
    .wrap_err("database migration failed")?;
```

Rules:
- Migrations are **irreversible by default** — write both up and down scripts
- Never edit a committed migration — add a new one instead
- Column names use `snake_case` matching Rust field names for direct mapping

## Compile-Time Query Verification

Use `sqlx::query!` and `sqlx::query_as!` macros for all queries:

```rust
// Requires DATABASE_URL set at compile time (via .env)
let row = sqlx::query_as!(
    TaskRow,
    r#"
    SELECT id, payload, status, worker_id, attempts, scheduled_at, created_at
    FROM tasks
    WHERE id = $1
    "#,
    id.as_uuid()
)
.fetch_optional(&self.pool)
.await?;
```

The `DATABASE_URL` must be set in `.env` for `cargo build` to succeed. CI uses `cargo sqlx prepare` to generate an offline query cache (`sqlx-data.json`) — committed to the repo so CI builds don't need a live database:

```bash
# Generate offline query cache
cargo sqlx prepare --workspace
```

## Row-to-Domain Mapping

Database row types are **never** exposed outside the `infrastructure` crate. Map explicitly in the adapter:

```rust
// crates/infrastructure/src/adapters/postgres_task_repository.rs

// Internal row type — stays in infrastructure
struct TaskRow {
    pub id: Uuid,
    pub payload: serde_json::Value,
    pub status: String,
    pub worker_id: Option<Uuid>,
    pub attempts: i32,
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<TaskRow> for Task {
    type Error = PostgresAdapterError;

    fn try_from(row: TaskRow) -> Result<Self, Self::Error> {
        Ok(Task {
            id: TaskId::from(row.id),
            payload: serde_json::from_value(row.payload)
                .map_err(|e| PostgresAdapterError::Mapping { reason: e.to_string() })?,
            status: TaskStatus::try_from(row.status.as_str())
                .map_err(|_| PostgresAdapterError::Mapping {
                    reason: format!("unknown status: {}", row.status)
                })?,
            // ...
        })
    }
}
```

Rules:
- `TaskRow` (or equivalent) is `pub(crate)` — never `pub`
- All domain types constructed from row data via `TryFrom`, never direct field assignment
- `sqlx` types (`chrono::DateTime`, `Uuid`) are converted to domain newtypes in the mapping step

## Transactions

Use explicit transactions for multi-step operations:

```rust
pub async fn claim_task(&self, id: &TaskId, worker: &WorkerId) -> Result<Task, TaskError> {
    let mut tx = self.pool.begin().await.map_err(PostgresAdapterError::from)?;

    let row = sqlx::query_as!(TaskRow, /* ... SELECT FOR UPDATE ... */, id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

    let task = row
        .ok_or(TaskError::NotFound { id: id.clone() })?
        .try_into()?;

    sqlx::query!(/* UPDATE status ... */, worker.as_uuid(), id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(PostgresAdapterError::from)?;

    tx.commit().await.map_err(PostgresAdapterError::from)?;
    Ok(task)
}
```

## Testing

Integration tests use `testcontainers` to spin up real PostgreSQL:

```rust
// crates/infrastructure/tests/task_repository_test.rs
use testcontainers::{clients::Cli, images::postgres::Postgres};

#[tokio::test]
async fn test_save_and_find_task() {
    let docker = Cli::default();
    let container = docker.run(Postgres::default());
    let pool = create_pool_for_test(&container).await;

    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();

    // test against real postgres
}
```

Unit tests of application logic mock the repository trait — they never need a real DB.

## Consequences

**Positive:**
- SQL errors caught at compile time — no runtime surprises in production
- Full SQL power — no ORM impedance mismatch for complex queries
- Domain types cleanly isolated from persistence types
- Migrations are versioned, tracked, and automatically applied

**Negative:**
- `DATABASE_URL` required at compile time — mitigated by `cargo sqlx prepare` offline mode
- More boilerplate than an ORM — accepted for the compile-time safety and SQL control

## References

- [SQLx documentation](https://docs.rs/sqlx)
- [SQLx offline mode](https://github.com/launchbadge/sqlx/blob/main/sqlx-cli/README.md#enable-building-in-offline-mode-with-query)
- [testcontainers-rs](https://docs.rs/testcontainers)
