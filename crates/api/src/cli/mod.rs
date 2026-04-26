//! CLI argument parsing and subcommand dispatch (ADR-0003).
//!
//! The top-level `Cli` struct holds global flags shared across all
//! subcommands. Server-specific flags live under the `Serve` variant.
//! When no subcommand is given, the binary defaults to `Serve`.

pub mod config;
pub mod db;
pub mod output;
pub mod submit;
pub mod tasks;
pub mod workers;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// iron-defer — durable background task execution engine.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Path to a TOML configuration file.
    ///
    /// Defaults to `config.toml` in the working directory.
    /// Overlay files (`config.{IRON_DEFER_PROFILE}.toml`) are loaded
    /// automatically when `IRON_DEFER_PROFILE` is set.
    #[arg(long, short = 'c', global = true, env = "IRON_DEFER_CONFIG")]
    pub config: Option<PathBuf>,

    /// `PostgreSQL` connection string.
    #[arg(long, global = true, env = "DATABASE_URL")]
    pub database_url: Option<String>,

    /// Emit output as JSON instead of human-readable tables.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP server, worker pool, and sweeper (default).
    Serve(Serve),
    /// Submit a task to a queue.
    Submit(submit::Submit),
    /// List and filter tasks.
    Tasks(tasks::Tasks),
    /// Show active worker status.
    Workers(workers::Workers),
    /// Configuration management.
    Config(config::Config),
}

/// Server mode flags — start the full engine.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct Serve {
    /// HTTP server listen port.
    #[arg(long, env = "PORT")]
    pub port: Option<u16>,

    /// Maximum simultaneous in-flight tasks.
    #[arg(long)]
    pub concurrency: Option<u32>,

    /// OTLP collector endpoint (empty = disabled).
    #[arg(long)]
    pub otlp_endpoint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_defaults_to_none() {
        let cli = Cli::parse_from(["iron-defer"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn serve_subcommand_parsed() {
        let cli = Cli::parse_from(["iron-defer", "serve", "--port", "9090"]);
        match cli.command {
            Some(Command::Serve(s)) => assert_eq!(s.port, Some(9090)),
            other => panic!("expected Serve, got {other:?}"),
        }
    }

    #[test]
    fn submit_subcommand_parsed() {
        let cli = Cli::parse_from([
            "iron-defer",
            "submit",
            "--queue",
            "payments",
            "--kind",
            "webhook",
            "--payload",
            r#"{"url":"https://example.com"}"#,
        ]);
        match cli.command {
            Some(Command::Submit(s)) => {
                assert_eq!(s.queue, "payments");
                assert_eq!(s.kind, "webhook");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn tasks_subcommand_with_filters() {
        let cli = Cli::parse_from([
            "iron-defer",
            "tasks",
            "--queue",
            "payments",
            "--status",
            "pending",
        ]);
        match cli.command {
            Some(Command::Tasks(t)) => {
                assert_eq!(t.queue.as_deref(), Some("payments"));
                assert_eq!(t.status.as_deref(), Some("pending"));
            }
            other => panic!("expected Tasks, got {other:?}"),
        }
    }

    #[test]
    fn workers_subcommand_parsed() {
        let cli = Cli::parse_from(["iron-defer", "workers"]);
        assert!(matches!(cli.command, Some(Command::Workers(_))));
    }

    #[test]
    fn config_validate_subcommand_parsed() {
        let cli = Cli::parse_from(["iron-defer", "config", "validate"]);
        match cli.command {
            Some(Command::Config(c)) => {
                assert!(matches!(c.action, config::ConfigAction::Validate));
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[test]
    fn global_json_flag_propagates() {
        let cli = Cli::parse_from(["iron-defer", "--json", "tasks"]);
        assert!(cli.json);
    }

    #[test]
    fn global_database_url_propagates() {
        let cli = Cli::parse_from(["iron-defer", "--database-url", "postgres://test", "workers"]);
        assert_eq!(cli.database_url.as_deref(), Some("postgres://test"));
    }
}
