//! `iron-defer tasks` subcommand — list and filter tasks.

use iron_defer_domain::{ListTasksFilter, QueueName, TaskError, TaskStatus};

use super::output;

/// List and filter tasks in the database.
#[derive(Debug, clap::Args)]
pub struct Tasks {
    /// Filter by queue name.
    #[arg(long)]
    pub queue: Option<String>,

    /// Filter by status (pending, running, completed, failed, cancelled, suspended).
    #[arg(long)]
    pub status: Option<String>,

    /// Maximum number of tasks to return.
    #[arg(long, default_value_t = 50)]
    pub limit: u32,

    /// Pagination offset.
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
}

fn parse_status(s: &str) -> Result<TaskStatus, String> {
    match s.to_ascii_lowercase().as_str() {
        "pending" => Ok(TaskStatus::Pending),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        "suspended" => Ok(TaskStatus::Suspended),
        other => Err(format!(
            "invalid status '{other}'; expected one of: pending, running, completed, failed, cancelled, suspended"
        )),
    }
}

/// Run the tasks subcommand.
///
/// # Errors
///
/// Prints errors to stderr and returns a non-zero exit code indicator.
pub async fn run(args: &Tasks, database_url: &str, json: bool) -> Result<(), i32> {
    let pool = super::db::cli_pool(database_url).await.map_err(|e| {
        output::print_error(&format!("database connection failed: {e}"), json);
        1
    })?;

    let queue = args
        .queue
        .as_deref()
        .map(QueueName::try_from)
        .transpose()
        .map_err(|e| {
            output::print_error(&format!("invalid queue name: {e}"), json);
            1
        })?;

    let status = args
        .status
        .as_deref()
        .map(parse_status)
        .transpose()
        .map_err(|e| {
            output::print_error(&e, json);
            1
        })?;

    let limit = args.limit.clamp(1, 100);

    let filter = ListTasksFilter {
        queue,
        status,
        limit,
        offset: args.offset,
    };

    let repo = std::sync::Arc::new(iron_defer_infrastructure::PostgresTaskRepository::new(pool, false))
        as std::sync::Arc<dyn iron_defer_application::TaskRepository>;

    let scheduler = iron_defer_application::SchedulerService::new(repo);

    let result = scheduler
        .list_tasks(&filter)
        .await
        .map_err(|e: TaskError| {
            output::print_error(&format!("query failed: {e}"), json);
            1
        })?;

    output::print_task_table(&result.tasks, result.total, limit, args.offset, json);
    Ok(())
}
