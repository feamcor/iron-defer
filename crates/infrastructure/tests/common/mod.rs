//! Shared testcontainers helper for infrastructure integration tests.
//!
//! Architecture §Process Patterns (testcontainers) mandates "one Postgres container per test
//! binary, never per test". The container handle and pool live in a
//! `OnceCell` so the first test that touches `test_pool()` boots the
//! container, runs migrations, and every subsequent test reuses the same
//! handle. The container drops when the test binary exits.
//!
//! **Prefer `fresh_pool_on_shared_container()`** for binaries with
//! multiple `#[tokio::test]` functions. Each test's Tokio runtime drop
//! can strand pool connections against the prior runtime's reactor,
//! causing `PoolTimedOut` in subsequent tests that share the same pool.
//! `fresh_pool_on_shared_container()` gives each test its own pool on
//! the shared container, isolating pool lifecycle.
//!
//! **Test data isolation:** because all tests share one database, each
//! test must scope its writes with a unique queue name (e.g.
//! `format!("test-{}", Uuid::new_v4())`) to avoid cross-test pollution.
//!
//! **Docker-unavailable behavior:** if Docker is not running locally, the
//! container start fails and `test_pool()` returns `None`. Each test
//! checks for `None` and returns early with an `eprintln!("[skip] ...")`
//! message rather than failing the binary.

use iron_defer_infrastructure::MIGRATOR;
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::sync::OnceCell;

#[allow(dead_code)] // each integration-test binary uses a different subset of helpers
static TEST_DB: OnceCell<Option<TestDb>> = OnceCell::const_new();

#[allow(dead_code)]
struct TestDb {
    pool: PgPool,
    url: String,
    // Held for the lifetime of the test binary; dropping kills the container.
    _container: ContainerAsync<Postgres>,
}

/// Lazily start a shared Postgres testcontainer and return a `&'static PgPool`.
///
/// Returns `None` if Docker is unavailable or container start fails. Tests
/// must check for `None` and skip cleanly.
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

async fn boot_test_db() -> Result<TestDb, Box<dyn std::error::Error + Send + Sync>> {
    let container = Postgres::default().start().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPool::connect(&url).await?;
    MIGRATOR.run(&pool).await?;
    Ok(TestDb {
        pool,
        url: format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres"),
        _container: container,
    })
}

/// Build a fresh `PgPool` connected to the shared test container, with
/// migrations already applied.
///
/// Preferred over [`test_pool`] for test binaries where multiple
/// `#[tokio::test]` functions run: each test's runtime drop can strand
/// pool-held connections against the prior runtime's reactor, and the
/// next test's `pool.acquire()` then hangs on `PoolTimedOut`.
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
