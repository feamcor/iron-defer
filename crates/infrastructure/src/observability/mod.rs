//! Observability adapters.
//!
//! Provides the JSON tracing subscriber and metrics wiring.
//! Architecture §Structure Patterns (module layout) pins this layout.
//!
//! # `init_tracing` feature gate
//!
//! [`tracing::init_tracing`] is re-exported only when the `bin-init`
//! Cargo feature is enabled (Architecture §Enforcement Guidelines — embedded library
//! callers must not install a global subscriber). `crates/api/Cargo.toml`
//! enables it for the standalone binary; the `iron-defer` library
//! façade does NOT. [`tracing::build_fmt_layer`] and
//! [`tracing::scrub_url`] remain un-gated — embedders composing their
//! own subscriber legitimately need those helpers.

pub mod metrics;
pub mod tracing;

pub use self::metrics::{create_metrics, register_pool_gauges};
pub use self::tracing::{build_fmt_layer, scrub_url};

#[cfg(feature = "bin-init")]
pub use self::metrics::init_metrics;
#[cfg(feature = "bin-init")]
pub use self::tracing::init_tracing;
