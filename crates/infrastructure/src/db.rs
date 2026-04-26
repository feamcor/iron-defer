//! Database connection helpers and embedded migration constant.
//!
//! Architecture references:
//! - §C3: `sqlx::migrate!("../../migrations")` baked into the library at
//!   compile time. Path is relative to this crate's `Cargo.toml`, which lives
//!   at `crates/infrastructure/`, so `../../migrations` resolves to the
//!   workspace-root `migrations/` directory.
//! - ADR-0005: `runtime-tokio-rustls` `SQLx` feature, no `OpenSSL`.
//! - The standalone pool default is 10 connections, encoded as
//!   `DEFAULT_MAX_CONNECTIONS`.

use std::time::Duration;

use iron_defer_application::DatabaseConfig;
use iron_defer_domain::TaskError;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::error::PostgresAdapterError;

/// Upper bound on error-source chain traversal in [`is_pool_timeout`].
///
/// Prevents infinite loops on pathological circular error chains. 16 hops
/// is well beyond any realistic `sqlx` → adapter → domain nesting depth.
const MAX_ERROR_CHAIN_DEPTH: usize = 16;

/// Default pool size when `DatabaseConfig::max_connections` is `0`.
///
/// Mirrors the `IRON_DEFER_POOL_SIZE` standalone-mode default documented in
/// the PRD (line 336). See also [`MAX_POOL_CONNECTIONS`] for the ceiling.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// Hard ceiling on the connection pool size (FR41).
///
/// A 4-worker engine with sweeper, HTTP server, and pool overhead can
/// reasonably need 20+ connections. A ceiling of 100 prevents runaway
/// configuration while allowing legitimate high-throughput deployments.
pub const MAX_POOL_CONNECTIONS: u32 = 100;

/// Default `acquire_timeout` for `PgPoolOptions`.
///
/// Fixed default used when no explicit acquire timeout is configured.
pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Default `min_connections` for `PgPoolOptions`.
///
/// Zero lets the pool go fully cold during Postgres outages so the runtime
/// does not burn CPU trying to re-establish a connection floor while the
/// database is unreachable. Cold-start latency is masked by the first
/// claim's `acquire_timeout` budget. See `docs/guidelines/postgres-reconnection.md`.
pub const DEFAULT_MIN_CONNECTIONS: u32 = 0;

/// Default `idle_timeout` for `PgPoolOptions`.
///
/// Idle connections are recycled after five minutes so stale TCP sessions
/// do not silently linger across Postgres restarts. Combined with
/// `test_before_acquire(true)`, this lets the pool transparently replace
/// broken connections without a custom reconnection loop.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Default `max_lifetime` for `PgPoolOptions`.
///
/// Upper bound on any pooled connection's age, regardless of idleness.
/// Guards against slow TCP half-open states and any server-side
/// connection-age caps configured on Postgres.
pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_mins(30);

/// Embedded migration set, baked into the library binary at compile time.
///
/// The library API surface (`crates/api/src/lib.rs`) invokes
/// `MIGRATOR.run(&pool)` from `IronDefer::build()`. Tests also use this
/// migrator through the shared test helpers.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Manually repair a dirty migration state by removing failed records
/// from the `_sqlx_migrations` table.
///
/// Use this if a migration failed partway and left the database in a
/// state where `MIGRATOR.run()` refuses to proceed.
///
/// # Errors
///
/// Returns [`PostgresAdapterError::Query`] if the database command fails.
pub async fn repair_migrations(pool: &sqlx::PgPool) -> Result<(), PostgresAdapterError> {
    sqlx::query("DELETE FROM _sqlx_migrations WHERE success = false")
        .execute(pool)
        .await
        .map_err(|e| PostgresAdapterError::Query { source: e })?;
    Ok(())
}

/// Returns a `PgPoolOptions` pre-configured with the hardened defaults
/// used by the standalone engine. Embedded callers who provide their own
/// pool via `IronDeferBuilder::pool()` can use this as a starting point
/// and further customize before calling `.connect()`.
#[must_use]
pub fn recommended_pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(DEFAULT_MAX_CONNECTIONS)
        .min_connections(DEFAULT_MIN_CONNECTIONS)
        .acquire_timeout(DEFAULT_ACQUIRE_TIMEOUT)
        .idle_timeout(Some(DEFAULT_IDLE_TIMEOUT))
        .max_lifetime(Some(DEFAULT_MAX_LIFETIME))
        .test_before_acquire(true)
}

/// Construct a `PgPool` from a `DatabaseConfig`.
///
/// `max_connections` of `0` resolves to [`DEFAULT_MAX_CONNECTIONS`].
/// Uses `recommended_pool_options()` as the base, then applies config overrides.
///
/// # Errors
///
/// Returns `TaskError::Storage` if the underlying `sqlx` connect fails
/// (DNS, TCP, auth, TLS, etc.).
pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool, TaskError> {
    let max_connections = if config.max_connections == 0 {
        DEFAULT_MAX_CONNECTIONS
    } else {
        config.max_connections
    };

    if max_connections > MAX_POOL_CONNECTIONS {
        return Err(TaskError::Storage {
            source: format!(
                "configured pool size ({max_connections}) exceeds ceiling ({MAX_POOL_CONNECTIONS})"
            )
            .into(),
        });
    }

    let pool = recommended_pool_options()
        .max_connections(max_connections)
        .test_before_acquire(config.test_before_acquire)
        .connect(&config.url)
        .await
        .map_err(PostgresAdapterError::from)?;

    Ok(pool)
}

/// Return `true` if the given `TaskError` was caused by pool saturation
/// or a connectivity-class `sqlx` error.
///
/// Walks the `std::error::Error::source()` chain until it finds a
/// `sqlx::Error`, then matches variants that represent "operator-visible
/// but expected during an outage" conditions:
///
/// - `PoolTimedOut` — classic pool saturation (NFR-R6 target)
/// - `PoolClosed` — pool was closed under the caller
/// - `Io(_)` — raw TCP failure, typically a severed connection during outage
/// - `WorkerCrashed` — `SQLx` pool background worker died (outage side-effect)
/// - `Database(e)` where SQLSTATE class is `08` — connection exception per
///   the SQL standard (8000/8001/8003/8004/8006/8007/P01/...)
///
/// Used by the worker and sweeper to downgrade these errors from
/// `error!` to `warn!` tagged `event = "pool_saturated"`, keeping the
/// error log reserved for genuinely unexpected failures (auth, schema,
/// constraint violations). Implemented here rather than in `application`
/// to keep `sqlx` out of the `application` crate's dependency graph
/// (Architecture §Architectural Boundaries — Layer dependency rules).
#[must_use]
pub fn is_pool_timeout(err: &TaskError) -> bool {
    let mut current: &dyn std::error::Error = err;
    for _ in 0..MAX_ERROR_CHAIN_DEPTH {
        if let Some(PostgresAdapterError::DatabaseScrubbed { code, .. }) =
            current.downcast_ref::<PostgresAdapterError>()
        {
            return code.as_deref().is_some_and(|c| c.starts_with("08"));
        }
        if let Some(sqlx_err) = current.downcast_ref::<sqlx::Error>() {
            return match sqlx_err {
                sqlx::Error::PoolTimedOut
                | sqlx::Error::PoolClosed
                | sqlx::Error::Io(_)
                | sqlx::Error::WorkerCrashed => true,
                sqlx::Error::Database(db_err) => db_err.code().is_some_and(|c| c.starts_with("08")),
                _ => false,
            };
        }
        match current.source() {
            Some(next) => current = next,
            None => return false,
        }
    }
    false // Exceeded hop limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_connections_matches_prd() {
        // PRD line 336: standalone-mode default is 10.
        assert_eq!(DEFAULT_MAX_CONNECTIONS, 10);
    }

    #[test]
    fn default_acquire_timeout_is_finite() {
        assert!(DEFAULT_ACQUIRE_TIMEOUT.as_secs() > 0);
    }

    #[test]
    fn default_idle_timeout_is_finite() {
        assert!(DEFAULT_IDLE_TIMEOUT.as_secs() > 0);
    }

    #[test]
    fn default_max_lifetime_is_finite() {
        assert!(DEFAULT_MAX_LIFETIME.as_secs() > 0);
    }

    #[test]
    fn default_max_lifetime_exceeds_idle_timeout() {
        // max_lifetime must be >= idle_timeout; otherwise long-lived idle
        // connections would be recycled before their lifetime cap meant
        // anything.
        assert!(DEFAULT_MAX_LIFETIME >= DEFAULT_IDLE_TIMEOUT);
    }

    #[test]
    fn is_pool_timeout_detects_pool_timed_out() {
        let adapter_err: PostgresAdapterError = sqlx::Error::PoolTimedOut.into();
        let task_err: TaskError = adapter_err.into();
        assert!(is_pool_timeout(&task_err));
    }

    #[test]
    fn is_pool_timeout_rejects_other_sqlx_errors() {
        let adapter_err: PostgresAdapterError =
            sqlx::Error::Protocol("not a pool timeout".to_string()).into();
        let task_err: TaskError = adapter_err.into();
        assert!(!is_pool_timeout(&task_err));
    }

    #[test]
    fn is_pool_timeout_detects_pool_closed() {
        let adapter_err: PostgresAdapterError = sqlx::Error::PoolClosed.into();
        let task_err: TaskError = adapter_err.into();
        assert!(is_pool_timeout(&task_err));
    }

    #[test]
    fn is_pool_timeout_detects_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let adapter_err: PostgresAdapterError = sqlx::Error::Io(io_err).into();
        let task_err: TaskError = adapter_err.into();
        assert!(is_pool_timeout(&task_err));
    }

    #[test]
    fn is_pool_timeout_detects_worker_crashed() {
        let adapter_err: PostgresAdapterError = sqlx::Error::WorkerCrashed.into();
        let task_err: TaskError = adapter_err.into();
        assert!(is_pool_timeout(&task_err));
    }

    #[test]
    fn is_pool_timeout_rejects_non_storage_variants() {
        let task_err = TaskError::InvalidPayload {
            kind: iron_defer_domain::PayloadErrorKind::Validation {
                message: "nope".to_string(),
            },
        };
        assert!(!is_pool_timeout(&task_err));
    }

    #[tokio::test]
    async fn pool_size_ceiling_rejects_oversized() {
        let config = DatabaseConfig {
            url: "postgres://localhost/test".into(),
            max_connections: 101,
            ..Default::default()
        };
        let err = create_pool(&config).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exceeds ceiling"), "{msg}");
        assert!(msg.contains("101"), "{msg}");
        assert!(msg.contains("100"), "{msg}");
    }

    #[tokio::test]
    async fn pool_size_at_ceiling_accepted() {
        // This will fail at connect (no real DB), but must pass the ceiling check.
        let config = DatabaseConfig {
            url: "postgres://localhost:1/test".into(),
            max_connections: 100,
            ..Default::default()
        };
        let err = create_pool(&config).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("exceeds ceiling"),
            "should pass ceiling check but got: {msg}"
        );
    }

    #[tokio::test]
    async fn pool_size_zero_uses_default_passes_ceiling() {
        let config = DatabaseConfig {
            url: "postgres://localhost:1/test".into(),
            max_connections: 0,
            ..Default::default()
        };
        let err = create_pool(&config).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("exceeds ceiling"),
            "default (10) should pass ceiling: {msg}"
        );
    }

    #[test]
    fn is_pool_timeout_detects_database_scrubbed_class_08() {
        let adapter_err = PostgresAdapterError::DatabaseScrubbed {
            message: "server closed the connection unexpectedly".to_string(),
            code: Some("08006".to_string()),
        };
        let task_err: TaskError = adapter_err.into();
        assert!(
            is_pool_timeout(&task_err),
            "class-08 DatabaseScrubbed should be detected as pool timeout"
        );
    }

    #[test]
    fn is_pool_timeout_rejects_database_scrubbed_non_class_08() {
        let adapter_err = PostgresAdapterError::DatabaseScrubbed {
            message: "constraint violation".to_string(),
            code: Some("23514".to_string()),
        };
        let task_err: TaskError = adapter_err.into();
        assert!(
            !is_pool_timeout(&task_err),
            "non-class-08 DatabaseScrubbed should not be a pool timeout"
        );
    }

    #[test]
    fn is_pool_timeout_rejects_database_scrubbed_no_code() {
        let adapter_err = PostgresAdapterError::DatabaseScrubbed {
            message: "unknown error".to_string(),
            code: None,
        };
        let task_err: TaskError = adapter_err.into();
        assert!(
            !is_pool_timeout(&task_err),
            "DatabaseScrubbed without code should not be a pool timeout"
        );
    }

    #[test]
    fn is_pool_timeout_handles_deep_error_chain() {
        #[derive(Debug)]
        struct ChainLink {
            depth: usize,
            source: Option<Box<ChainLink>>,
        }
        impl std::fmt::Display for ChainLink {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "link-{}", self.depth)
            }
        }
        impl std::error::Error for ChainLink {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.source
                    .as_ref()
                    .map(|s| s.as_ref() as &dyn std::error::Error)
            }
        }

        fn build_chain(depth: usize) -> ChainLink {
            let mut link = ChainLink {
                depth: 0,
                source: None,
            };
            for d in 1..depth {
                link = ChainLink {
                    depth: d,
                    source: Some(Box::new(link)),
                };
            }
            link
        }

        let deep_chain = build_chain(20);
        let task_err = TaskError::Storage {
            source: Box::new(deep_chain),
        };
        assert!(
            !is_pool_timeout(&task_err),
            "deep chain exceeding MAX_ERROR_CHAIN_DEPTH must return false, not panic"
        );
    }

    #[test]
    fn default_min_connections_is_zero() {
        // Outage recovery requires a cold pool — otherwise the pool retries
        // to the unreachable Postgres on its own schedule while the DB is down.
        assert_eq!(DEFAULT_MIN_CONNECTIONS, 0);
    }

    #[test]
    fn recommended_pool_options_returns_valid_config() {
        let _opts = recommended_pool_options();
    }
}
