//! Shared testcontainers helper for api crate integration tests.
//!
//! Mirrors the 1A.2 infrastructure-crate `tests/common/mod.rs`. One
//! Postgres container per test binary, lazily started, runtime-skipped
//! if Docker is unavailable. Migrations run via the **public** API path
//! `iron_defer::IronDefer::migrator()` so the integration suite exercises
//! the migrator accessor surface itself.

pub mod e2e;
pub mod otel;

use iron_defer::IronDefer;
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::sync::OnceCell;
use uuid::Uuid;

#[allow(dead_code)]
static TEST_DB: OnceCell<Option<TestDb>> = OnceCell::const_new();

#[allow(dead_code)]
struct TestDb {
    pool: PgPool,
    url: String,
    _container: ContainerAsync<Postgres>,
}

/// Lazily start a shared Postgres testcontainer and return a `&'static PgPool`.
///
/// Returns `None` if Docker is unavailable or container start fails.
/// Tests must check for `None` and skip cleanly via `eprintln!("[skip] ...")`.
#[allow(dead_code)]
pub async fn test_pool() -> Option<&'static PgPool> {
    let cell = TEST_DB
        .get_or_init(|| async {
            match boot_test_db().await {
                Ok(db) => Some(db),
                Err(e) => {
                    assert!(
                        std::env::var("IRON_DEFER_REQUIRE_DB").is_err(),
                        "[testcontainers] IRON_DEFER_REQUIRE_DB is set but Postgres \
                         container failed: {e}"
                    );
                    eprintln!(
                        "[testcontainers] failed to start Postgres container: {e}. \
                         Integration tests requiring a database will be skipped."
                    );
                    None
                }
            }
        })
        .await;

    cell.as_ref().map(|db| &db.pool)
}

/// Lazily start a fresh Postgres container WITHOUT running migrations.
/// Used by the `builder_skip_migrations_does_not_run_migrator` test which
/// needs to assert that the `tasks` table does NOT exist after a
/// skip-migrations build.
///
/// **Important:** this helper allocates a NEW container each call (it
/// does NOT reuse the shared `TEST_DB` cell), because the shared
/// container has migrations applied permanently after the first test.
#[allow(dead_code)]
pub async fn fresh_unmigrated_pool() -> Option<(PgPool, ContainerAsync<Postgres>)> {
    match Postgres::default().start().await {
        Ok(container) => {
            let port = match container.get_host_port_ipv4(5432).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[testcontainers] failed to get host port for fresh container: {e}");
                    return None;
                }
            };
            let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
            match PgPool::connect(&url).await {
                Ok(pool) => Some((pool, container)),
                Err(e) => {
                    eprintln!("[testcontainers] failed to connect to fresh container: {e}");
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("[testcontainers] failed to start fresh container: {e}");
            None
        }
    }
}

async fn boot_test_db() -> Result<TestDb, Box<dyn std::error::Error + Send + Sync>> {
    let container = Postgres::default().start().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    // Larger pool for shared integration tests — multiple engines + sweepers
    // + workers + migration runs can saturate the default 10-connection pool.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(40)
        .connect(&url)
        .await?;
    // Run migrations via the PUBLIC API path — exercises IronDefer::migrator()
    // accessor surface as part of the integration suite.
    IronDefer::migrator().run(&pool).await?;
    Ok(TestDb {
        pool,
        url,
        _container: container,
    })
}

/// Build a fresh `PgPool` connected to the shared test container, with
/// migrations already applied.
///
/// Preferred over [`test_pool`] for test binaries where multiple
/// `#[tokio::test]` functions start worker engines: each test's runtime
/// drop can strand pool-held connections against the prior runtime's
/// reactor, and the next test's `pool.acquire()` then hangs 30 s on
/// `PoolTimedOut`. Each test instead gets its own pool whose internal
/// actor is spawned onto the caller's runtime, isolating pool lifecycle
/// from cross-test runtime churn.
///
/// Returns `None` if Docker is unavailable (mirrors [`test_pool`]).
#[allow(dead_code)]
pub async fn fresh_pool_on_shared_container() -> Option<PgPool> {
    let cell = TEST_DB
        .get_or_init(|| async {
            match boot_test_db().await {
                Ok(db) => Some(db),
                Err(e) => {
                    eprintln!(
                        "[testcontainers] failed to start Postgres container: {e}. \
                         Integration tests requiring a database will be skipped."
                    );
                    None
                }
            }
        })
        .await;

    let url = cell.as_ref().map(|db| db.url.as_str())?;
    match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await
    {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("[testcontainers] failed to build per-test pool: {e}");
            None
        }
    }
}

/// Generate a unique queue name for test scoping.
#[allow(dead_code)]
pub fn unique_queue() -> String {
    format!("test-{}", Uuid::new_v4())
}

/// Return the database URL for the shared test container.
///
/// Returns `None` if Docker is unavailable.
#[allow(dead_code)]
pub async fn test_db_url() -> Option<&'static str> {
    let cell = TEST_DB
        .get_or_init(|| async {
            match boot_test_db().await {
                Ok(db) => Some(db),
                Err(e) => {
                    eprintln!(
                        "[testcontainers] failed to start Postgres container: {e}. \
                         Integration tests requiring a database will be skipped."
                    );
                    None
                }
            }
        })
        .await;

    cell.as_ref().map(|db| db.url.as_str())
}
