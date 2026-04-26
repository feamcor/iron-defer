//! `iron-defer workers` subcommand — show active worker status.

use iron_defer_domain::TaskError;

use super::output;

/// Show active worker status.
#[derive(Debug, clap::Args)]
pub struct Workers;

/// Run the workers subcommand.
///
/// # Errors
///
/// Prints errors to stderr and returns a non-zero exit code indicator.
pub async fn run(database_url: &str, json: bool) -> Result<(), i32> {
    let pool = super::db::cli_pool(database_url).await.map_err(|e| {
        output::print_error(&format!("database connection failed: {e}"), json);
        1
    })?;

    let repo = std::sync::Arc::new(iron_defer_infrastructure::PostgresTaskRepository::new(pool, false))
        as std::sync::Arc<dyn iron_defer_application::TaskRepository>;

    let scheduler = iron_defer_application::SchedulerService::new(repo);

    let workers = scheduler.worker_status().await.map_err(|e: TaskError| {
        output::print_error(&format!("query failed: {e}"), json);
        1
    })?;

    output::print_worker_table(&workers, json);
    Ok(())
}
