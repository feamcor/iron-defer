//! Integration tests for configuration validation (Story 5.2).
//!
//! These tests validate the full stack: config → pool creation → ceiling
//! check, and config → builder → mutual exclusion check.

use iron_defer::{DatabaseConfig, IronDefer};
use iron_defer_infrastructure::create_pool;

#[tokio::test]
async fn pool_ceiling_rejects_oversized_via_create_pool() {
    let config = DatabaseConfig {
        url: "postgres://localhost:1/test".into(),
        max_connections: 101,
        ..Default::default()
    };
    let err = create_pool(&config)
        .await
        .expect_err("pool size above ceiling must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("exceeds ceiling"),
        "expected ceiling error, got: {msg}"
    );
}

#[tokio::test]
async fn unlogged_audit_mutual_exclusion_via_builder() {
    let config = DatabaseConfig {
        unlogged_tables: true,
        audit_log: true,
        ..Default::default()
    };
    let result = IronDefer::builder().database_config(config).build().await;
    let err = result.expect_err("UNLOGGED + audit_log must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("mutually exclusive"),
        "expected mutual exclusion error, got: {msg}"
    );
}
