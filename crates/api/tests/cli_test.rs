//! CLI subcommand integration tests (Story 4.3).
//!
//! Exercises the `run_*` functions directly (not via process spawning)
//! to share the test database connection.

mod common;

use iron_defer::cli;

// ---------------------------------------------------------------------------
// Submit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submit_creates_task_in_db() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let Some(url) = common::test_db_url().await else {
        return;
    };
    let queue = common::unique_queue();

    let submit = cli::submit::Submit {
        queue: queue.clone(),
        kind: "test_submit".into(),
        payload: r#"{"hello":"world"}"#.into(),
        scheduled_at: None,
        priority: 5,
        max_attempts: None,
        idempotency_key: None,
    };

    let result = cli::submit::run(&submit, url, false).await;
    assert!(result.is_ok(), "submit should succeed: {result:?}");

    let filter = iron_defer_domain::ListTasksFilter {
        queue: Some(iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap()),
        status: None,
        limit: 10,
        offset: 0,
    };
    let repo = std::sync::Arc::new(iron_defer_infrastructure::PostgresTaskRepository::new(pool, false))
        as std::sync::Arc<dyn iron_defer_application::TaskRepository>;
    let sched = iron_defer_application::SchedulerService::new(repo);
    let result = sched.list_tasks(&filter).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.tasks[0].kind().as_ref(), "test_submit");
    assert_eq!(result.tasks[0].priority().get(), 5);
}

#[tokio::test]
async fn submit_with_scheduled_at() {
    let Some(_pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let Some(url) = common::test_db_url().await else {
        return;
    };
    let queue = common::unique_queue();

    let submit = cli::submit::Submit {
        queue,
        kind: "test_scheduled".into(),
        payload: "{}".into(),
        scheduled_at: Some("2030-01-01T00:00:00Z".into()),
        priority: 0,
        max_attempts: None,
        idempotency_key: None,
    };

    let result = cli::submit::run(&submit, url, false).await;
    assert!(result.is_ok(), "submit with scheduled_at should succeed");
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tasks_lists_created_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let Some(url) = common::test_db_url().await else {
        return;
    };
    let queue = common::unique_queue();

    let repo = std::sync::Arc::new(iron_defer_infrastructure::PostgresTaskRepository::new(pool, false))
        as std::sync::Arc<dyn iron_defer_application::TaskRepository>;
    let sched = iron_defer_application::SchedulerService::new(repo);
    let qn = iron_defer_domain::QueueName::try_from(queue.as_str()).unwrap();

    for i in 0..3 {
        sched
            .enqueue_raw(
                &qn,
                &format!("kind_{i}"),
                serde_json::json!({}),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
    }

    let args = cli::tasks::Tasks {
        queue: Some(queue),
        status: None,
        limit: 50,
        offset: 0,
    };

    let result = cli::tasks::run(&args, url, false).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn tasks_invalid_status_returns_error() {
    let Some(_pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let Some(url) = common::test_db_url().await else {
        return;
    };

    let args = cli::tasks::Tasks {
        queue: None,
        status: Some("bogus".into()),
        limit: 50,
        offset: 0,
    };

    let result = cli::tasks::run(&args, url, false).await;
    assert!(result.is_err(), "invalid status should fail");
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workers_returns_empty_when_no_running_tasks() {
    let Some(_pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let Some(url) = common::test_db_url().await else {
        return;
    };

    let result = cli::workers::run(url, false).await;
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// Config validate
// ---------------------------------------------------------------------------

#[test]
fn config_validate_with_defaults_succeeds() {
    let path = std::path::PathBuf::from("/dev/null/nonexistent.toml");
    let result = cli::config::run_validate(Some(&path), None, false);
    assert!(result.is_ok(), "default config should validate: {result:?}");
}

// NOTE: the env-var-based invalid config test is omitted from the
// integration suite because `set_var` in Rust 2024 is `unsafe` and
// environment mutation in a multi-threaded test binary races with other
// tests reading env vars. The WorkerConfig::validate() rejection is
// thoroughly covered by unit tests in application/config.rs.
