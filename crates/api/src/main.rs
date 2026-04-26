//! iron-defer standalone binary entry point.
//!
//! Dispatches CLI subcommands: `serve` (default), `submit`, `tasks`,
//! `workers`, `config validate`. The figment chain (ADR-0003) applies
//! to `serve`; other subcommands use the database URL directly.

#![forbid(unsafe_code)]

use clap::Parser;
use iron_defer::cli::{Cli, Command, Serve};
use iron_defer::config;
use iron_defer::shutdown::{shutdown_meter_provider, shutdown_tracer_provider};
use iron_defer_infrastructure::{init_metrics, init_tracing};
use opentelemetry::metrics::MeterProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    color_eyre::install()?;

    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Serve(_)) => {
            let serve = match cli.command {
                Some(Command::Serve(ref s)) => s.clone(),
                _ => Serve::default(),
            };
            run_serve(&cli, &serve)?;
        }
        Some(Command::Submit(ref submit)) => {
            let db_url = require_database_url(&cli);
            let rt = build_runtime()?;
            let code = rt
                .block_on(async { iron_defer::cli::submit::run(submit, &db_url, cli.json).await });
            if let Err(exit_code) = code {
                std::process::exit(exit_code);
            }
        }
        Some(Command::Tasks(ref tasks)) => {
            let db_url = require_database_url(&cli);
            let rt = build_runtime()?;
            let code =
                rt.block_on(async { iron_defer::cli::tasks::run(tasks, &db_url, cli.json).await });
            if let Err(exit_code) = code {
                std::process::exit(exit_code);
            }
        }
        Some(Command::Workers(_)) => {
            let db_url = require_database_url(&cli);
            let rt = build_runtime()?;
            let code =
                rt.block_on(async { iron_defer::cli::workers::run(&db_url, cli.json).await });
            if let Err(exit_code) = code {
                std::process::exit(exit_code);
            }
        }
        Some(Command::Config(ref cfg)) => {
            use iron_defer::cli::config::ConfigAction;
            match cfg.action {
                ConfigAction::Validate => {
                    if let Err(exit_code) = iron_defer::cli::config::run_validate(
                        cli.config.as_ref(),
                        cli.database_url.as_deref(),
                        cli.json,
                    ) {
                        std::process::exit(exit_code);
                    }
                }
            }
        }
    }

    Ok(())
}

fn run_serve(cli: &Cli, serve: &Serve) -> Result<(), Box<dyn std::error::Error>> {
    let app_config = config::load(
        cli.config.as_ref(),
        cli.database_url.as_deref(),
        Some(serve),
    )?;

    init_tracing(&app_config.observability)?;

    let (meter_provider, prom_registry) = init_metrics(&app_config.observability)?;
    let meter = meter_provider.meter("iron_defer");
    let metrics = iron_defer_infrastructure::create_metrics(&meter);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        concurrency = app_config.worker.concurrency,
        port = app_config.server.port,
        "iron-defer starting"
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        let pool = iron_defer_infrastructure::create_pool(&app_config.database).await?;

        let engine = iron_defer::IronDefer::builder()
            .pool(pool)
            .worker_config(app_config.worker.clone())
            .database_config(app_config.database.clone())
            .metrics(metrics)
            .prometheus_registry(prom_registry)
            .readiness_timeout(std::time::Duration::from_secs(
                app_config.server.readiness_timeout_secs,
            ))
            .build()
            .await?;

        let engine = std::sync::Arc::new(engine);
        let token = iron_defer::CancellationToken::new();

        let engine_bg = engine.clone();
        let worker_token = token.clone();
        let worker_handle = tokio::spawn(async move {
            let _ = engine_bg.start(worker_token).await;
        });

        let router = iron_defer::http::router::build(engine.clone());
        let bind = format!(
            "{}:{}",
            if app_config.server.bind_address.is_empty() {
                "0.0.0.0"
            } else {
                &app_config.server.bind_address
            },
            app_config.server.port
        );
        let listener = tokio::net::TcpListener::bind(&bind).await?;
        tracing::info!(%bind, "HTTP server listening");

        let server_token = token.clone();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(server_token.cancelled_owned())
                .await
                .expect("HTTP server");
        });

        iron_defer::shutdown::shutdown_signal().await;
        tracing::info!("shutdown signal received, draining...");
        token.cancel();

        if tokio::time::timeout(app_config.worker.shutdown_timeout, async {
            let _ = tokio::join!(worker_handle, server_handle);
        })
        .await
        .is_err()
        {
            tracing::warn!(
                timeout_ms = app_config.worker.shutdown_timeout.as_millis(),
                "Graceful shutdown timed out; process exiting abruptly"
            );
        }

        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    shutdown_meter_provider(|| meter_provider.shutdown());
    shutdown_tracer_provider();
    Ok(())
}

fn require_database_url(cli: &Cli) -> String {
    if let Some(ref url) = cli.database_url {
        return url.clone();
    }
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.is_empty()
    {
        return url;
    }
    iron_defer::cli::output::print_error(
        "DATABASE_URL is required; set via --database-url flag or DATABASE_URL env var",
        cli.json,
    );
    std::process::exit(1);
}

fn build_runtime() -> Result<tokio::runtime::Runtime, Box<dyn std::error::Error>> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}
