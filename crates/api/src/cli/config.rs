//! `iron-defer config` subcommand — configuration management.

use std::path::PathBuf;

use iron_defer_application::AppConfig;

use super::output;

/// Configuration management.
#[derive(Debug, clap::Args)]
pub struct Config {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, clap::Subcommand)]
pub enum ConfigAction {
    /// Validate the configuration chain and print a summary.
    Validate,
}

/// Run the config validate subcommand.
///
/// # Errors
///
/// Prints errors to stderr and returns exit code 2 on validation failure.
pub fn run_validate(
    config_path: Option<&PathBuf>,
    database_url: Option<&str>,
    json: bool,
) -> Result<(), i32> {
    let app_config = load_config(config_path, database_url).map_err(|e| {
        output::print_error(&format!("configuration load failed: {e}"), json);
        2
    })?;

    if let Err(e) = app_config.worker.validate() {
        output::print_error(&format!("worker config invalid: {e}"), json);
        return Err(2);
    }

    output::print_config_summary(&app_config, json);
    Ok(())
}

fn load_config(
    config_path: Option<&PathBuf>,
    database_url: Option<&str>,
) -> color_eyre::Result<AppConfig> {
    use figment::Figment;
    use figment::providers::{Env, Format, Serialized, Toml};

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

    let config: AppConfig = figment.extract()?;
    Ok(config)
}
