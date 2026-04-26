//! Graceful shutdown signal handling.
//!
//! Architecture references:
//! - §D6.1: `CancellationToken` tree, drain timeout
//! - §Process Patterns (shutdown.rs responsibilities): OS signal handling,
//!   root `CancellationToken`, drain timeout enforcement
//!
//! This is a first-class orchestration component, not a utility module.

/// Wait for a shutdown signal (SIGTERM or `ctrl_c`).
///
/// Returns only when a genuine OS signal is received. If a signal handler
/// cannot be installed, the failure is logged and that arm of the select
/// becomes never-ready rather than resolving spuriously — callers relying
/// on this future to trigger shutdown won't see a false cancel.
///
/// # Example
///
/// ```rust,no_run
/// use tokio_util::sync::CancellationToken;
///
/// # async fn example() {
/// let token = CancellationToken::new();
/// let token_clone = token.clone();
/// tokio::spawn(async move {
///     iron_defer::shutdown::shutdown_signal().await;
///     token_clone.cancel();
/// });
/// # }
/// ```
pub async fn shutdown_signal() {
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "failed to listen for ctrl_c signal");
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("received ctrl_c signal, initiating shutdown");
        }
        () = terminate => {
            tracing::info!("received SIGTERM signal, initiating shutdown");
        }
    }
}

/// Flush and shut down the global `OTel` tracer provider.
///
/// Ensures traces are flushed before exit.
pub fn shutdown_tracer_provider() {
    shutdown_observability(|| {
        opentelemetry::global::shutdown_tracer_provider();
        Ok::<(), String>(())
    });
}

/// Flush buffered `OTel` metric exports on process shutdown.
///
/// Must be called AFTER the engine drain completes (so any final metric
/// emissions are captured) and BEFORE process exit (so any OTLP
/// `PeriodicReader` flushes its send queue). This must be wired through
/// to be wired through the shutdown flow rather than dropped silently in
/// `main`. The argument is a closure so this helper stays independent of
/// `opentelemetry_sdk` types — the embedded library crate does not carry
/// an SDK dependency.
pub fn shutdown_meter_provider<F, E>(shutdown_fn: F)
where
    F: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    shutdown_observability(shutdown_fn);
}

/// Generic wrapper for observability provider shutdown.
pub fn shutdown_observability<F, E>(shutdown_fn: F)
where
    F: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    match shutdown_fn() {
        Ok(()) => tracing::info!("observability provider flushed"),
        Err(e) => tracing::error!(error = %e, "observability provider shutdown failed"),
    }
}
