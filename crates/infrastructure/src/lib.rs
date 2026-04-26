//! iron-defer infrastructure crate.
//!
//! Adapters that satisfy the application-layer port traits and bridge to
//! external systems (`PostgreSQL`, `OTel`, …).
//!
//! Per the architecture rule "no logic in lib.rs", this file only declares
//! and re-exports modules.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod db;
pub(crate) mod error;
pub mod observability;

pub use crate::adapters::{PostgresCheckpointWriter, PostgresTaskRepository};
pub use crate::db::{
    DEFAULT_ACQUIRE_TIMEOUT, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_LIFETIME,
    DEFAULT_MIN_CONNECTIONS, MAX_POOL_CONNECTIONS, MIGRATOR, create_pool, is_pool_timeout,
    repair_migrations,
};
pub use crate::observability::{build_fmt_layer, create_metrics, register_pool_gauges, scrub_url};

// `init_tracing` and `init_metrics` are gated behind the `bin-init` feature
// so only binaries that opt in get initialization helpers at the crate root.
#[cfg(feature = "bin-init")]
pub use crate::observability::{init_metrics, init_tracing};
