//! `iron-defer submit` subcommand — submit a task to a queue.

use chrono::{DateTime, FixedOffset};
use iron_defer_domain::{QueueName, TaskError};

use super::output;

/// Submit a task to a named queue.
#[derive(Debug, clap::Args)]
pub struct Submit {
    /// Target queue name.
    #[arg(long)]
    pub queue: String,

    /// Task type discriminator (kind).
    #[arg(long)]
    pub kind: String,

    /// JSON payload string.
    #[arg(long)]
    pub payload: String,

    /// Future scheduling timestamp (ISO 8601).
    #[arg(long)]
    pub scheduled_at: Option<String>,

    /// Task priority (higher = picked sooner). Default: 0.
    #[arg(long, default_value_t = 0)]
    pub priority: i16,

    /// Maximum retry attempts. Default: server default (3).
    #[arg(long)]
    pub max_attempts: Option<i32>,

    /// Idempotency key for exactly-once submission.
    #[arg(long)]
    pub idempotency_key: Option<String>,
}

/// Run the submit subcommand.
///
/// Connects directly to Postgres via `SchedulerService` (no `TaskRegistry`
/// needed — CLI is an operator tool, not an engine instance).
///
/// # Errors
///
/// Prints errors to stderr and returns a non-zero exit code indicator.
pub async fn run(submit: &Submit, database_url: &str, json: bool) -> Result<(), i32> {
    let pool = super::db::cli_pool(database_url).await.map_err(|e| {
        output::print_error(&format!("database connection failed: {e}"), json);
        1
    })?;

    let payload: serde_json::Value = serde_json::from_str(&submit.payload).map_err(|e| {
        output::print_error(&format!("invalid JSON payload: {e}"), json);
        1
    })?;

    let queue = QueueName::try_from(submit.queue.as_str()).map_err(|e| {
        output::print_error(&format!("invalid queue name: {e}"), json);
        1
    })?;

    let scheduled_at = submit
        .scheduled_at
        .as_deref()
        .map(|s| {
            DateTime::<FixedOffset>::parse_from_rfc3339(s)
                .map(|dt| dt.to_utc())
                .map_err(|e| {
                    output::print_error(
                        &format!("invalid --scheduled-at (expected ISO 8601): {e}"),
                        json,
                    );
                    1
                })
        })
        .transpose()?;

    let max_attempts = if let Some(ma) = submit.max_attempts {
        if ma < 1 {
            output::print_error("--max-attempts must be >= 1", json);
            return Err(1);
        }
        Some(ma)
    } else {
        None
    };

    let repo = std::sync::Arc::new(iron_defer_infrastructure::PostgresTaskRepository::new(pool, false))
        as std::sync::Arc<dyn iron_defer_application::TaskRepository>;

    let scheduler = iron_defer_application::SchedulerService::new(repo);

    let record = if let Some(ref idempotency_key) = submit.idempotency_key {
        // Default retention of 24h as a fallback if not configured
        let retention = std::time::Duration::from_secs(24 * 60 * 60);
        let (rec, created) = scheduler
            .enqueue_raw_idempotent(
                &queue,
                &submit.kind,
                payload,
                scheduled_at,
                Some(submit.priority),
                max_attempts,
                idempotency_key,
                retention,
                None,
                None,
            )
            .await
            .map_err(|e: TaskError| {
                output::print_error(&format!("submit failed: {e}"), json);
                1
            })?;
        if !created {
            if json {
                // In JSON mode, we just return the record, but we could add a field if needed.
                // For now, we'll return exit code 2 as requested.
            } else {
                eprintln!("duplicate idempotency key — returning existing task");
            }
            output::print_task_record(&rec, json);
            return Err(2);
        }
        rec
    } else {
        scheduler
            .enqueue_raw(
                &queue,
                &submit.kind,
                payload,
                scheduled_at,
                Some(submit.priority),
                max_attempts,
                None,
                None,
            )
            .await
            .map_err(|e: TaskError| {
                output::print_error(&format!("submit failed: {e}"), json);
                1
            })?
    };

    output::print_task_record(&record, json);
    Ok(())
}
