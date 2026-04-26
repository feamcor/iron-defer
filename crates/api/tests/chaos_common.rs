//! Shared helpers for chaos tests. Each chaos test spins its own isolated
//! Postgres container — MUST NOT reference `tests/common/mod.rs` `TEST_DB`.

use iron_defer::IronDefer;
use iron_defer_application::DatabaseConfig;
use iron_defer_infrastructure::create_pool;
use sqlx::PgPool;
use testcontainers::core::IntoContainerPort;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Start a fresh isolated Postgres container, run migrations, and return
/// `(pool, container, url, port)`.
///
/// The host port is pinned so `container.stop() → container.start()` preserves
/// the mapping (required for `SQLx` reconnection tests).
///
/// # Panics
///
/// Panics if Docker is unavailable or migrations fail.
pub async fn boot_isolated_chaos_db() -> (PgPool, ContainerAsync<Postgres>, String, u16) {
    let port = {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port for chaos test");
        let p = listener.local_addr().expect("read local_addr").port();
        drop(listener);
        p
    };

    let container = Postgres::default()
        .with_mapped_port(port, 5432.tcp())
        .start()
        .await
        .expect(
            "[testcontainers] Docker is required for chaos tests; \
             set IRON_DEFER_SKIP_DOCKER_CHAOS=1 to skip locally",
        );
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let cfg = DatabaseConfig {
        url: url.clone(),
        max_connections: 10,
        ..Default::default()
    };
    let pool = create_pool(&cfg)
        .await
        .expect("create pool against isolated Postgres");

    IronDefer::migrator()
        .run(&pool)
        .await
        .expect("run migrations on isolated Postgres");

    (pool, container, url, port)
}

/// Returns `true` if the `IRON_DEFER_SKIP_DOCKER_CHAOS` env var is set,
/// indicating chaos tests should be skipped.
#[must_use]
pub fn should_skip() -> bool {
    std::env::var("IRON_DEFER_SKIP_DOCKER_CHAOS").is_ok()
}

/// Generate a unique queue name for test scoping.
#[must_use]
pub fn unique_queue() -> String {
    format!("chaos-{}", uuid::Uuid::new_v4())
}
