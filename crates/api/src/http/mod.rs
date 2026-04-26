//! HTTP transport layer — axum router, handlers, and error mapping.
//!
//! Architecture references:
//! - §Structure Patterns (module layout): `router.rs`, `handlers/`, `errors.rs`
//! - §D4.1: no authentication in MVP
//! - §D4.2: 1 MiB request body limit via `DefaultBodyLimit`
//! - §Format Patterns: response shape and HTTP status code mapping

pub mod errors;
pub mod extractors;
pub mod handlers;
pub mod router;
