//! Tracing subscriber initialization + URL scrubbing helper.
//!
//! This module is the sole home of `tracing-subscriber` usage in
//! iron-defer — Architecture §Structure Patterns (module layout) mandates the location.
//! The embedded library (`IronDefer::builder`) MUST NOT invoke anything
//! here; the caller owns the subscriber (Architecture §Enforcement Guidelines). Only
//! the standalone binary (`crates/api/src/main.rs`) calls
//! [`init_tracing`].
//!
//! # Composability
//!
//! [`build_fmt_layer`] returns the pre-configured JSON `fmt::Layer` as
//! `impl Layer<S>` so callers can compose it alongside an `OTel` layer
//! on the same `Registry` without rewriting this module. The fmt-layer
//! field shape stays stable across both subscribers, preventing
//! field-name drift when telemetry layers are added.
//!
//! # Payload privacy (FR38 / NFR-S2)
//!
//! [`scrub_url`] redacts the password segment of libpq-style connection
//! strings so `sqlx::Error::Configuration` payloads cannot leak a DB URL
//! through a `tracing::instrument(err)` serialization chain. The
//! infrastructure `PostgresAdapterError::from(sqlx::Error)` conversion
//! wires it in at the single Configuration branch.

use iron_defer_application::ObservabilityConfig;
use iron_defer_domain::TaskError;
use opentelemetry::global;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::TracerProvider;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, fmt, layer::SubscriberExt, registry::LookupSpan,
    util::SubscriberInitExt,
};

/// Build the shared JSON formatter layer used by [`init_tracing`] and
/// OTel-enriched initializers.
///
/// Returned as `impl Layer<S>` so the caller can compose it with an
/// `EnvFilter` and any additional layers (e.g. `tracing_opentelemetry`)
/// on a single [`Registry`], preserving field layout across initializers.
///
/// Field configuration:
/// - `with_current_span(true)` + `with_span_list(true)` — surfaces
///   `#[instrument]` fields (`task_id`, `worker_id`, `queue`, `kind`) at the
///   top level of every JSON record.
/// - `with_target(true)` — retains module path for log-aggregator routing.
/// - `flatten_event(true)` — inlines event fields into the record root.
/// - `.with_writer(std::io::stdout)` — FR19 MVP sink per PRD line 366.
pub fn build_fmt_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_target(true)
        .flatten_event(true)
        .with_writer(std::io::stdout)
}

/// Install the production JSON tracing subscriber.
///
/// Composes `Registry` + `EnvFilter` (from `RUST_LOG`, falling back to
/// `info`) + [`build_fmt_layer`] and installs them as the global
/// dispatcher via `try_init`.
///
/// Emits exactly one `info!` record on success (`tracing subscriber
/// initialized`) carrying the effective filter directive so operators
/// can verify their runtime log-level choice.
///
/// # Architecture rules enforced
///
/// - The embedded library (`IronDefer::builder`) MUST NOT call this
///   function. The caller owns their subscriber (Architecture §Enforcement Guidelines —
///   by extension, installing a global subscriber inside a library
///   function is forbidden).
/// - Only the standalone binary in `crates/api/src/main.rs` is a valid
///   caller.
///
/// # Parameter
///
/// The [`ObservabilityConfig`] reference allows this initializer to control
/// OTLP wiring without changing the public function signature.
///
/// # Errors
///
/// Returns [`TaskError::Storage`] wrapping the `tracing-subscriber`
/// `TryInitError` when a global dispatcher is already installed. This
/// can happen in tests that re-enter the function or in embedded mode
/// where the caller has already wired their own subscriber — in both
/// cases a panic would be the wrong response.
pub fn init_tracing(config: &ObservabilityConfig) -> Result<(), TaskError> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let filter_directive = env_filter.to_string();

    let registry = Registry::default().with(env_filter).with(build_fmt_layer());

    #[cfg(feature = "tokio_console")]
    let registry = registry.with(console_subscriber::ConsoleLayer::builder().spawn());

    registry.try_init().map_err(|e| TaskError::Storage {
        source: Box::new(e),
    })?;

    if !config.otlp_endpoint.is_empty() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&config.otlp_endpoint)
            .build()
            .map_err(|e| TaskError::Storage {
                source: Box::new(e),
            })?;
        let tracer_provider = TracerProvider::builder()
            .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
            .build();
        global::set_tracer_provider(tracer_provider);
    } else {
        let tracer_provider = TracerProvider::builder().build();
        global::set_tracer_provider(tracer_provider);
    }

    tracing::info!(
        filter = %filter_directive,
        json = true,
        tokio_console = cfg!(feature = "tokio_console"),
        otlp_traces = !config.otlp_endpoint.is_empty(),
        "tracing subscriber initialized"
    );
    Ok(())
}

/// Redact the password segment of a libpq-style connection URL.
///
/// Transforms `postgres://user:PASSWORD@host/db` into
/// `postgres://user:***@host/db`. Leaves input unchanged when:
/// - the string does not contain `"://"` (not a URL)
/// - the userinfo does not include a `:` (no password present)
/// - the authority does not include `@` (no userinfo at all)
///
/// Used by the `PostgresAdapterError::from(sqlx::Error::Configuration)`
/// conversion path to prevent DB URLs from leaking through
/// `tracing::instrument(err)` serialization chains (NFR-S2,
/// Architecture §D4.3).
///
/// This is a narrow-scope scrub: only the password segment of a
/// detected URL is redacted. It does not attempt full structural scrubbing
/// of arbitrary `sqlx::Error::Database` payloads.
#[must_use]
pub fn scrub_url(s: &str) -> String {
    let Some((scheme, rest)) = s.split_once("://") else {
        return s.to_owned();
    };

    // Locate the userinfo/host boundary
    // via `rfind('@')` over the FULL `rest` BEFORE clipping the authority
    // at '?' or '#'. A password containing '?' or '#' (libpq accepts both)
    // would otherwise truncate `authority_span` inside the password, and
    // the subsequent `rfind('@')` would return None — leaking the original
    // URL verbatim. Path-'/' is preserved as before (`rfind('@')` still
    // crosses it), and `?`/'#' are clipped only in the post-@ slice where
    // they unambiguously terminate the authority per RFC 3986.
    let Some(at_pos) = rest.rfind('@') else {
        return s.to_owned(); // No userinfo — nothing to scrub
    };

    let userinfo = &rest[..at_pos];
    let after_at = &rest[at_pos..]; // starts with '@'

    let host_end = after_at.find(['?', '#']).unwrap_or(after_at.len());
    let host_onwards = &after_at[..host_end];
    let trailing = &after_at[host_end..];

    let Some(colon_pos) = userinfo.find(':') else {
        return s.to_owned(); // No password segment to redact
    };

    // Empty password (`user:`) — nothing
    // to scrub. Returning `***` would falsely imply a password was present.
    if colon_pos + 1 == userinfo.len() {
        return s.to_owned();
    }

    format!(
        "{}://{}:***{}{}",
        scheme,
        &userinfo[..colon_pos],
        host_onwards,
        trailing
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // scrub_url — pure-function tests (no global state touched)
    // -------------------------------------------------------------------

    #[test]
    fn scrub_url_redacts_password() {
        assert_eq!(
            scrub_url("postgres://u:p@h/d"),
            "postgres://u:***@h/d".to_string()
        );
    }

    #[test]
    fn scrub_url_leaves_plain_host_unchanged() {
        // No '@' in authority — nothing to scrub.
        assert_eq!(
            scrub_url("postgres://host/db"),
            "postgres://host/db".to_string()
        );
        // '@' but no ':' in userinfo — nothing to scrub.
        assert_eq!(
            scrub_url("postgres://user@host/db"),
            "postgres://user@host/db".to_string()
        );
    }

    #[test]
    fn scrub_url_handles_malformed_input() {
        // No '://' — passthrough.
        assert_eq!(scrub_url("not a url"), "not a url".to_string());
        assert_eq!(scrub_url(""), String::new());
    }

    #[test]
    fn scrub_url_handles_long_password_with_special_chars() {
        // Real-world-ish password with special characters.
        assert_eq!(
            scrub_url("postgres://admin:P@ss!word:123@db.internal:5432/app"),
            "postgres://admin:***@db.internal:5432/app".to_string()
        );
    }

    #[test]
    fn scrub_url_preserves_path_and_query_like_text() {
        assert_eq!(
            scrub_url("postgresql://u:secret@host:5432/mydb?sslmode=require"),
            "postgresql://u:***@host:5432/mydb?sslmode=require".to_string()
        );
    }

    #[test]
    fn scrub_url_handles_authority_only() {
        assert_eq!(
            scrub_url("postgres://u:p@h"),
            "postgres://u:***@h".to_string()
        );
    }

    #[test]
    fn scrub_url_redacts_password_containing_slash() {
        // A password with a raw '/' must not
        // be leaked by splitting authority/path at the first '/'. The
        // rfind('@') strategy locates the userinfo terminator correctly.
        assert_eq!(
            scrub_url("postgres://u:p/w@h/d"),
            "postgres://u:***@h/d".to_string()
        );
    }

    #[test]
    fn scrub_url_preserves_query_string_after_authority() {
        // '?' terminates the authority — password scrub must run only
        // over the authority span, and the query string must survive.
        assert_eq!(
            scrub_url("postgres://u:secret@host:5432?sslmode=require"),
            "postgres://u:***@host:5432?sslmode=require".to_string()
        );
    }

    #[test]
    fn scrub_url_redacts_password_containing_question_mark() {
        // libpq accepts raw '?' in
        // passwords. The scrub must not clip authority at '?' before
        // locating '@', otherwise `rfind('@')` on the truncated span
        // returns None and the full URL (including the password) leaks.
        assert_eq!(
            scrub_url("postgres://u:p?ass@h/d"),
            "postgres://u:***@h/d".to_string()
        );
    }

    #[test]
    fn scrub_url_redacts_password_containing_hash() {
        // Same failure mode as '?',
        // applied to '#' (RFC 3986 fragment delimiter).
        assert_eq!(
            scrub_url("postgres://u:p#ass@h/d"),
            "postgres://u:***@h/d".to_string()
        );
    }

    #[test]
    fn scrub_url_leaves_empty_password_unchanged() {
        // `postgres://u:@h/d` has an
        // empty password segment — returning `***` would falsely imply a
        // password was present. Pass through unchanged.
        assert_eq!(
            scrub_url("postgres://u:@h/d"),
            "postgres://u:@h/d".to_string()
        );
    }

    // NOTE: `init_tracing_returns_error_on_double_init` lives in
    // `crates/infrastructure/tests/init_tracing_test.rs`. Installing a
    // global tracing subscriber conflicts with `#[tracing_test::traced_test]`
    // (which also calls `set_global_default` and panics on conflict), so
    // the global-state test runs in its own integration-test binary.
}
