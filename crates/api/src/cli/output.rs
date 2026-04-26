//! CLI output formatting — human-readable tables and JSON.

use iron_defer_application::AppConfig;
use iron_defer_domain::{TaskRecord, WorkerStatus};

fn short_uuid(id: &impl std::fmt::Display) -> String {
    let s = id.to_string();
    if s.len() > 12 {
        format!("{}...", &s[..8])
    } else {
        s
    }
}

fn format_status(status: iron_defer_domain::TaskStatus) -> &'static str {
    match status {
        iron_defer_domain::TaskStatus::Pending => "pending",
        iron_defer_domain::TaskStatus::Running => "running",
        iron_defer_domain::TaskStatus::Completed => "completed",
        iron_defer_domain::TaskStatus::Failed => "failed",
        iron_defer_domain::TaskStatus::Cancelled => "cancelled",
        iron_defer_domain::TaskStatus::Suspended => "suspended",
        _ => "unknown",
    }
}

pub fn print_task_record(record: &TaskRecord, json: bool) {
    if json {
        match serde_json::to_string_pretty(record) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize task record: {e}"),
        }
        return;
    }

    println!("Task submitted successfully:");
    println!("  ID:           {}", record.id());
    println!("  Queue:        {}", record.queue());
    println!("  Kind:         {}", record.kind());
    println!("  Status:       {}", format_status(record.status()));
    println!("  Priority:     {}", record.priority());
    println!(
        "  Attempts:     {}/{}",
        record.attempts(),
        record.max_attempts()
    );
    println!("  Scheduled At: {}", record.scheduled_at().to_rfc3339());
    println!("  Created At:   {}", record.created_at().to_rfc3339());
}

pub fn print_task_table(tasks: &[TaskRecord], total: u64, limit: u32, offset: u32, json: bool) {
    if json {
        let obj = serde_json::json!({
            "tasks": tasks,
            "total": total,
            "limit": limit,
            "offset": offset,
        });
        match serde_json::to_string_pretty(&obj) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize task list: {e}"),
        }
        return;
    }

    if tasks.is_empty() {
        println!("No tasks found. (total: {total})");
        return;
    }

    println!(
        "{:<12} {:<20} {:<12} {:<10} CREATED_AT",
        "ID", "KIND", "STATUS", "ATTEMPTS"
    );
    println!("{}", "-".repeat(78));

    for task in tasks {
        println!(
            "{:<12} {:<20} {:<12} {:<10} {}",
            short_uuid(&task.id()),
            task.kind(),
            format_status(task.status()),
            format!("{}/{}", task.attempts(), task.max_attempts()),
            task.created_at().to_rfc3339(),
        );
    }

    println!();
    let total_present = tasks.len() as u64;
    if total_present == 0 {
        println!("Showing 0 of {total} tasks");
    } else {
        let start = u64::from(offset).saturating_add(1).min(total);
        let end = u64::from(offset).saturating_add(total_present).min(total);
        println!("Showing {start}-{end} of {total} tasks");
    }
}

pub fn print_worker_table(workers: &[WorkerStatus], json: bool) {
    if json {
        match serde_json::to_string_pretty(workers) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize worker status: {e}"),
        }
        return;
    }

    if workers.is_empty() {
        println!("No active workers.");
        return;
    }

    println!("{:<12} {:<20} TASKS_IN_FLIGHT", "WORKER_ID", "QUEUE");
    println!("{}", "-".repeat(50));

    for w in workers {
        println!(
            "{:<12} {:<20} {}",
            short_uuid(&w.worker_id),
            w.queue,
            w.tasks_in_flight,
        );
    }
}

fn mask_database_url(url: &str) -> String {
    if url.is_empty() {
        return "(not set)".to_string();
    }
    if let Some(at_pos) = url.rfind('@') {
        let scheme_end = url.find("://").map_or(0, |p| p + 3);
        format!("{}***@***", &url[..scheme_end.min(at_pos)])
    } else {
        "***".to_string()
    }
}

pub fn print_config_summary(config: &AppConfig, json: bool) {
    if json {
        let summary = serde_json::json!({
            "database": {
                "url": mask_database_url(&config.database.url),
                "max_connections": config.database.max_connections,
            },
            "server": {
                "port": config.server.port,
            },
            "worker": {
                "concurrency": config.worker.concurrency,
                "log_payload": config.worker.log_payload,
            },
            "observability": {
                "otlp_endpoint": if config.observability.otlp_endpoint.is_empty() {
                    "(disabled)"
                } else {
                    &config.observability.otlp_endpoint
                },
            },
            "status": "valid",
        });
        match serde_json::to_string_pretty(&summary) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize config summary: {e}"),
        }
        return;
    }

    println!("Configuration valid:");
    println!(
        "  Database URL:     {}",
        mask_database_url(&config.database.url)
    );
    println!("  Max Connections:  {}", config.database.max_connections);
    println!("  Server Port:      {}", config.server.port);
    println!("  Concurrency:      {}", config.worker.concurrency);
    println!("  Log Payload:      {}", config.worker.log_payload);
    let otlp = if config.observability.otlp_endpoint.is_empty() {
        "(disabled)"
    } else {
        &config.observability.otlp_endpoint
    };
    println!("  OTLP Endpoint:    {otlp}");
}

pub fn print_error(msg: &str, json: bool) {
    if json {
        let obj = serde_json::json!({ "error": msg });
        if let Ok(s) = serde_json::to_string_pretty(&obj) {
            eprintln!("{s}");
        } else {
            eprintln!("error: {msg}");
        }
    } else {
        eprintln!("error: {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_database_url_hides_credentials() {
        assert_eq!(
            mask_database_url("postgres://user:pass@host:5432/db"),
            "postgres://***@***"
        );
    }

    #[test]
    fn mask_database_url_handles_empty() {
        assert_eq!(mask_database_url(""), "(not set)");
    }

    #[test]
    fn mask_database_url_handles_no_at_sign() {
        assert_eq!(mask_database_url("some-opaque-string"), "***");
    }

    #[test]
    fn mask_database_url_handles_at_in_password() {
        assert_eq!(
            mask_database_url("postgres://user:p@ss@host:5432/db"),
            "postgres://***@***"
        );
    }

    #[test]
    fn short_uuid_truncates_long_ids() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(short_uuid(&id), "550e8400...");
    }

    fn make_test_record() -> TaskRecord {
        use iron_defer_domain::*;
        let now = chrono::Utc::now();
        TaskRecord::builder()
            .id(TaskId::new())
            .queue(QueueName::try_from("test").unwrap())
            .kind(TaskKind::try_from("test_kind").unwrap())
            .payload(std::sync::Arc::new(serde_json::json!({})))
            .status(TaskStatus::Pending)
            .priority(iron_defer_domain::Priority::new(0))
            .attempts(iron_defer_domain::AttemptCount::ZERO)
            .max_attempts(iron_defer_domain::MaxAttempts::DEFAULT)
            .scheduled_at(now)
            .created_at(now)
            .updated_at(now)
            .build()
    }

    #[test]
    fn print_task_table_offset_beyond_total_does_not_panic() {
        let tasks = vec![make_test_record()];
        print_task_table(&tasks, 50, 10, 100, false);
    }

    #[test]
    fn print_task_table_last_page_clamps_end() {
        let tasks = vec![make_test_record(), make_test_record()];
        print_task_table(&tasks, 3, 10, 2, false);
    }
}
