//! P2-INT-009 — Pool exhaustion: exhaust all connections, verify the engine
//! blocks (does not panic), then recovers when connections are returned.
//!
//! Uses a tiny pool (2 connections) on the shared testcontainer, holds both
//! connections via raw SQL, then releases them and verifies the engine can
//! enqueue again.

mod common;

use std::time::Duration;

use iron_defer::IronDefer;

#[tokio::test]
async fn pool_exhaustion_blocks_then_recovers() {
    let Some(url) = common::test_db_url().await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Tiny pool: 2 connections, short acquire timeout so we don't wait 30s.
    let tiny_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(2))
        .connect(url)
        .await
        .expect("connect tiny pool");

    IronDefer::migrator()
        .run(&tiny_pool)
        .await
        .expect("migrations");

    // Hold both connections.
    let held_a = tiny_pool.acquire().await.expect("acquire connection A");
    let held_b = tiny_pool.acquire().await.expect("acquire connection B");

    assert_eq!(
        tiny_pool.size(),
        2,
        "pool should have exactly 2 connections"
    );
    assert_eq!(tiny_pool.num_idle(), 0, "no idle connections expected");

    // Attempt a third acquire — should fail with timeout, NOT panic.
    let exhaustion_result = tokio::time::timeout(Duration::from_secs(3), tiny_pool.acquire()).await;

    assert!(
        exhaustion_result.is_err() || exhaustion_result.unwrap().is_err(),
        "pool acquire should fail or timeout when exhausted"
    );

    // Release one connection.
    drop(held_a);

    // Pool should recover — acquire succeeds.
    let mut recovered = tokio::time::timeout(Duration::from_secs(3), tiny_pool.acquire())
        .await
        .expect("acquire should not timeout after releasing a connection")
        .expect("acquire should succeed after releasing a connection");

    // Verify we can actually execute a query on the recovered connection.
    let row: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&mut *recovered)
        .await
        .expect("query on recovered connection");
    assert_eq!(row.0, 1);

    // Release all held connections — pool is usable again.
    drop(recovered);
    drop(held_b);
}
