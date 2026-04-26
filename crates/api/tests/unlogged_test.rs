mod common;

use std::sync::Arc;
use std::time::Duration;

use iron_defer::{DatabaseConfig, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serial_test::serial;
use sqlx::PgPool;

use common::e2e::{self, E2eTask};

fn fast_worker_config() -> WorkerConfig {
    WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(50),
        base_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(2),
        ..WorkerConfig::default()
    }
}

async fn query_table_persistence(pool: &PgPool, table: &str) -> Option<String> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT relpersistence::text FROM pg_class WHERE relname = $1",
    )
    .bind(table)
    .fetch_optional(pool)
    .await
    .expect("query pg_class");
    row.map(|r| r.0)
}

/// AC2: Startup rejected when both unlogged_tables and audit_log are true.
#[tokio::test]
async fn unlogged_mutual_exclusion_rejects_startup() {
    let cfg = DatabaseConfig {
        unlogged_tables: true,
        audit_log: true,
        ..DatabaseConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        err.contains("mutually exclusive"),
        "expected mutual exclusion error, got: {err}"
    );
}

/// Task 2.2: Confirm engine startup fails when both flags are true.
#[tokio::test]
async fn unlogged_mutual_exclusion_rejects_engine_build() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let result = IronDefer::builder()
        .pool(pool)
        .register::<E2eTask>()
        .database_config(DatabaseConfig {
            unlogged_tables: true,
            audit_log: true,
            ..DatabaseConfig::default()
        })
        .skip_migrations(true)
        .build()
        .await;

    assert!(result.is_err(), "should reject both flags");
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("mutually exclusive"),
        "error should mention mutual exclusion: {err}"
    );
}

/// AC1: Engine with unlogged_tables=true converts the table to UNLOGGED.
#[tokio::test]
#[serial]
async fn unlogged_tables_flag_converts_table() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Table should start as LOGGED (from normal migrations)
    let persistence = query_table_persistence(&pool, "tasks").await;
    assert_eq!(persistence.as_deref(), Some("p"), "tasks table should be LOGGED initially");

    // Build engine with unlogged_tables=true
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .database_config(DatabaseConfig {
            unlogged_tables: true,
            ..DatabaseConfig::default()
        })
        .skip_migrations(true)
        .build()
        .await
        .expect("build unlogged engine");

    assert!(engine.is_unlogged_tables());

    // Table should now be UNLOGGED
    let persistence = query_table_persistence(&pool, "tasks").await;
    assert_eq!(persistence.as_deref(), Some("u"), "tasks table should be UNLOGGED after build");

    // Restore to LOGGED for other tests sharing this container.
    // Build a LOGGED engine which handles FK constraint restoration.
    let _restore = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .database_config(DatabaseConfig::default())
        .skip_migrations(true)
        .build()
        .await
        .expect("restore to LOGGED");
}

/// Verify table can be restored from UNLOGGED to LOGGED.
#[tokio::test]
#[serial]
async fn unlogged_to_logged_restores_wal() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Convert to UNLOGGED via engine
    let _engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .database_config(DatabaseConfig {
            unlogged_tables: true,
            ..DatabaseConfig::default()
        })
        .skip_migrations(true)
        .build()
        .await
        .expect("build unlogged engine");
    let persistence = query_table_persistence(&pool, "tasks").await;
    assert_eq!(persistence.as_deref(), Some("u"));

    // Build engine with unlogged_tables=false — should restore to LOGGED
    let _engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .database_config(DatabaseConfig {
            unlogged_tables: false,
            ..DatabaseConfig::default()
        })
        .skip_migrations(true)
        .build()
        .await
        .expect("build logged engine");

    let persistence = query_table_persistence(&pool, "tasks").await;
    assert_eq!(persistence.as_deref(), Some("p"), "tasks table should be LOGGED after restore");
}

/// AC1: Basic operations work identically in UNLOGGED mode.
#[tokio::test]
#[serial]
async fn unlogged_mode_basic_operations() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .worker_config(fast_worker_config())
        .database_config(DatabaseConfig {
            unlogged_tables: true,
            ..DatabaseConfig::default()
        })
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build unlogged engine");

    let engine = Arc::new(engine);
    let token = iron_defer::CancellationToken::new();

    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Enqueue and verify task completes
    let record = engine
        .enqueue(&queue, E2eTask { data: "unlogged-test".into() })
        .await
        .expect("enqueue");

    let client = reqwest::Client::new();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let result = e2e::wait_for_status(
        &client,
        &base_url,
        &record.id().to_string(),
        "completed",
        Duration::from_secs(15),
    )
    .await;
    assert_eq!(result["status"], "completed");

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;

    // Restore to LOGGED via engine build (handles FK constraint restoration)
    let _restore = IronDefer::builder()
        .pool(pool.clone())
        .register::<E2eTask>()
        .database_config(DatabaseConfig::default())
        .skip_migrations(true)
        .build()
        .await
        .expect("restore to LOGGED");
}

/// AC2: Verify the unlogged_tables flag is accepted when audit_log is false.
#[tokio::test]
async fn unlogged_tables_flag_accepted() {
    let cfg = DatabaseConfig {
        unlogged_tables: true,
        audit_log: false,
        ..DatabaseConfig::default()
    };
    cfg.validate().expect("unlogged alone should be valid");
}
