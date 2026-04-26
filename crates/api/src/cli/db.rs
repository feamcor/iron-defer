//! Shared database connection helper for CLI subcommands.

use iron_defer_domain::TaskError;
use sqlx::PgPool;

/// Create a lightweight connection pool for CLI commands.
///
/// CLI commands are short-lived: they connect, run one query, and exit.
/// The pool uses minimal connections and a short acquire timeout.
///
/// Runs migrations on connect to ensure schema compatibility.
///
/// # Errors
///
/// Returns `TaskError::Storage` if the connection or migration fails.
pub async fn cli_pool(database_url: &str) -> Result<PgPool, TaskError> {
    if database_url.is_empty() {
        return Err(TaskError::Storage {
            source: "DATABASE_URL is required; set via --database-url flag or DATABASE_URL env var"
                .into(),
        });
    }

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
        .map_err(|e| TaskError::Storage {
            source: Box::new(e),
        })?;

    iron_defer_infrastructure::MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| TaskError::Storage {
            source: Box::new(e),
        })?;

    Ok(pool)
}
