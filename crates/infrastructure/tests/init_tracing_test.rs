//! Global-state integration test for `init_tracing`.
//!
//! This runs in its own binary to isolate the global tracing subscriber
//! it installs from other tests that use `#[tracing_test::traced_test]`
//! (both call `set_global_default`, which panics on conflict — see
//! `crates/infrastructure/src/observability/tracing.rs`).

use iron_defer_application::ObservabilityConfig;
// Story 3.1 second-pass review (P17 / D3-a): `init_tracing` is re-exported
// at the crate root only when the `bin-init` Cargo feature is enabled. Use
// the full module path here so the test compiles without requiring the
// workspace to enable the feature on the infrastructure crate (the
// function itself is unconditionally `pub`, only the convenience re-export
// is gated).
use iron_defer_infrastructure::observability::tracing::init_tracing;

#[test]
fn init_tracing_returns_error_on_double_init() {
    // Story 3.1 second-pass review (P5): the first call's outcome IS
    // part of the test. The prior `let _ = ...` form allowed the test
    // to pass even when the first init failed for an unrelated reason
    // (second call would then also fail, satisfying the assertion
    // without ever verifying the double-init guard). This binary is
    // isolated from other global-subscriber users by the integration-
    // test binary boundary (documented in the module comment above),
    // so the first call MUST succeed — anything else is a genuine
    // regression we want to observe.
    init_tracing(&ObservabilityConfig::default())
        .expect("first init_tracing must succeed in an isolated test binary");
    let result = init_tracing(&ObservabilityConfig::default());
    assert!(
        result.is_err(),
        "expected Err on second init (global already set), got {result:?}"
    );
}
