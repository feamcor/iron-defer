//! Configuration loading chain (ADR-0003).
//!
//! Precedence (lowest → highest):
//! 1. Compiled defaults (`AppConfig::default()`)
//! 2. Base config file (`config.toml`)
//! 3. Profile overlay (`config.{IRON_DEFER_PROFILE}.toml`)
//! 4. `.env` file (loaded by dotenvy before figment)
//! 5. Environment variables (`IRON_DEFER__` prefix, `__` separator)
//! 6. CLI flags (always win)

use std::path::PathBuf;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use iron_defer_application::AppConfig;

use crate::cli::Serve;

/// Load configuration from the full precedence chain.
///
/// `config_path` and `database_url` come from the top-level `Cli` global
/// flags. `serve` provides server-specific overrides (port, concurrency,
/// `otlp_endpoint`) and is `None` for non-serve subcommands.
///
/// # Errors
///
/// Returns an error if the merged configuration cannot be extracted
/// into `AppConfig` (missing required fields, type mismatches, etc.).
pub fn load(
    config_path: Option<&PathBuf>,
    database_url: Option<&str>,
    serve: Option<&Serve>,
) -> color_eyre::Result<AppConfig> {
    dotenvy::dotenv().ok();

    let path = config_path.cloned().unwrap_or_else(|| "config.toml".into());

    let profile = std::env::var("IRON_DEFER_PROFILE").unwrap_or_default();

    let mut figment = Figment::new()
        .merge(Serialized::defaults(AppConfig::default()))
        .merge(Toml::file(&path));

    if !profile.is_empty() {
        figment = figment.merge(Toml::file(format!("config.{profile}.toml")));
    }

    figment = figment.merge(Env::prefixed("IRON_DEFER__").split("__"));

    if let Some(url) = database_url {
        figment = figment.merge(Serialized::default("database.url", url));
    }

    if let Some(s) = serve {
        figment = apply_serve_overrides(figment, s);
    }

    let config: AppConfig = figment.extract()?;
    Ok(config)
}

fn apply_serve_overrides(mut figment: Figment, serve: &Serve) -> Figment {
    if let Some(port) = serve.port {
        figment = figment.merge(Serialized::default("server.port", port));
    }
    if let Some(concurrency) = serve.concurrency {
        figment = figment.merge(Serialized::default("worker.concurrency", concurrency));
    }
    if let Some(ref endpoint) = serve.otlp_endpoint {
        figment = figment.merge(Serialized::default("observability.otlp_endpoint", endpoint));
    }
    figment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_config_file() {
        let path = Some(PathBuf::from("/dev/null/nonexistent.toml"));
        let cfg = load(path.as_ref(), None, None).expect("defaults should load");
        assert_eq!(cfg.worker.concurrency, 4);
        assert_eq!(cfg.server.port, 0);
        assert!(cfg.database.url.is_empty());
    }

    #[test]
    fn missing_database_url_loads_with_empty_string() {
        let path = Some(PathBuf::from("/dev/null/nonexistent.toml"));
        let cfg = load(path.as_ref(), None, None)
            .expect("config load must not panic without DATABASE_URL");
        assert!(
            cfg.database.url.is_empty(),
            "database.url should be empty when DATABASE_URL is unset"
        );
    }

    #[test]
    fn cli_overrides_win() {
        let path = Some(PathBuf::from("/dev/null/nonexistent.toml"));
        let serve = Serve {
            port: Some(9999),
            concurrency: Some(16),
            otlp_endpoint: Some("http://otel:4317".into()),
        };
        let cfg = load(
            path.as_ref(),
            Some("postgres://cli@localhost/test"),
            Some(&serve),
        )
        .expect("cli overrides should load");
        assert_eq!(cfg.database.url, "postgres://cli@localhost/test");
        assert_eq!(cfg.server.port, 9999);
        assert_eq!(cfg.worker.concurrency, 16);
        assert_eq!(cfg.observability.otlp_endpoint, "http://otel:4317");
    }
}
