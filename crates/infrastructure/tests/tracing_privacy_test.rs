//! Story 3.1 AC 6 — DB URL privacy guard for pool construction failures.
//!
//! Runs in its own integration-test binary (separate global dispatcher
//! from the other infra integration tests) with the
//! `tracing-test = { features = ["no-env-filter"] }` feature so
//! `tracing_test::traced_test` captures logs from
//! `iron_defer_infrastructure` regardless of the test binary's crate
//! name (per the `tracing-test` 0.2 docs).

use iron_defer_application::DatabaseConfig;
use iron_defer_infrastructure::create_pool;

/// NFR-S2 / FR38: an invalid `DatabaseConfig::url` with a password in
/// clear text must NOT leak that password into any tracing log line.
///
/// Story 3.1 second-pass review (P6): the prior test used an
/// unreachable loopback port, which yields `sqlx::Error::Io` and never
/// enters the `sqlx::Error::Configuration` branch where `scrub_url` /
/// `scrub_message` live. The assertion passed only because sqlx's `Io`
/// Display does not echo the URL — not because the scrub logic fired.
///
/// This rewrite exercises the `Configuration` path in two ways:
/// 1. A malformed URL that sqlx rejects at parse time with
///    `Error::Configuration` (guaranteed scrub-path coverage).
/// 2. The original unreachable-host guard (kept as a separate assertion)
///    so future sqlx changes that DO echo the URL in `Io` errors still
///    get caught.
///
/// Both paths walk the error source chain through a tracing subscriber
/// before asserting the password is absent from the captured output.
#[tokio::test]
#[tracing_test::traced_test]
async fn tracing_captures_no_secrets_on_pool_construction_failure() {
    // Path 1 — malformed URL that sqlx will reject with
    // `Error::Configuration` at parse time, guaranteeing the scrub path
    // in `PostgresAdapterError::from` actually executes. A non-numeric
    // port is the canonical libpq parse failure and carries the full
    // URL (including the password) in the inner error.
    let bad_url_cfg = DatabaseConfig {
        url: "postgres://user:supersecret@localhost:notanumber/mydb".to_string(),
        max_connections: 1,
        ..Default::default()
    };
    let result = create_pool(&bad_url_cfg).await;
    assert!(result.is_err(), "create_pool should fail for malformed URL");
    if let Err(e) = &result {
        tracing::error!(error = %e, "pool construction failed (malformed URL fixture)");
        let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
        while let Some(s) = src {
            tracing::error!(source = %s, "error source (malformed URL)");
            src = s.source();
        }
    }

    // Path 2 — unreachable loopback port. The error path differs (Io
    // instead of Configuration), but the same leak-guard applies: the
    // password must never surface, regardless of which sqlx variant is
    // returned. This preserves the original coverage as a belt-and-
    // braces check against future sqlx changes.
    let unreachable_cfg = DatabaseConfig {
        url: "postgres://user:supersecret@127.0.0.1:1/nonexistent".to_string(),
        max_connections: 1,
        ..Default::default()
    };
    let result = create_pool(&unreachable_cfg).await;
    assert!(
        result.is_err(),
        "create_pool should fail for port-1 loopback"
    );
    if let Err(e) = &result {
        tracing::error!(error = %e, "pool construction failed (unreachable host fixture)");
        let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
        while let Some(s) = src {
            tracing::error!(source = %s, "error source (unreachable host)");
            src = s.source();
        }
    }

    assert!(
        !logs_contain("supersecret"),
        "DB password leaked into log output — pool-construction secret-scrub guard FAILED"
    );
}
