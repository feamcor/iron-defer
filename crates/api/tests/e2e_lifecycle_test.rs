//! E2E lifecycle tests — verify the complete task lifecycle through
//! library API, REST API, and CLI interfaces (Story 8.2, AC 2).

mod common;

use common::e2e::{self, E2eTask};
use serde_json::json;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn e2e_lifecycle_library_api() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let record = server
        .engine
        .enqueue(&queue, E2eTask { data: "lib-api".into() })
        .await
        .expect("enqueue");

    let client = reqwest::Client::new();
    let body = e2e::wait_for_status(
        &client,
        &server.base_url,
        &record.id().to_string(),
        "completed",
        TIMEOUT,
    )
    .await;

    assert_eq!(body["queue"], queue.as_str());
    assert_eq!(body["kind"], "e2e_test");
    assert_eq!(body["status"], "completed");

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_lifecycle_rest_api() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_test",
            "payload": {"data": "rest-api"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 201);
    let post_body: serde_json::Value = resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    let body =
        e2e::wait_for_status(&client, &server.base_url, task_id, "completed", TIMEOUT).await;

    assert_eq!(body["queue"], queue.as_str());
    assert_eq!(body["kind"], "e2e_test");
    assert_eq!(body["status"], "completed");

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_lifecycle_cli() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let output = assert_cmd::Command::cargo_bin("iron-defer")
        .expect("find binary")
        .env("DATABASE_URL", &server.db_url)
        .args([
            "--json",
            "submit",
            "--queue",
            &queue,
            "--kind",
            "e2e_test",
            "--payload",
            r#"{"data":"cli-test"}"#,
        ])
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI submit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cli_output: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse CLI JSON");
    let task_id = cli_output["id"].as_str().expect("CLI task id");

    let client = reqwest::Client::new();
    let body =
        e2e::wait_for_status(&client, &server.base_url, task_id, "completed", TIMEOUT).await;

    assert_eq!(body["status"], "completed");
    assert_eq!(body["kind"], "e2e_test");

    let list_output = assert_cmd::Command::cargo_bin("iron-defer")
        .expect("find binary")
        .env("DATABASE_URL", &server.db_url)
        .args([
            "--json",
            "tasks",
            "--queue",
            &queue,
            "--status",
            "completed",
        ])
        .output()
        .expect("run CLI tasks");
    assert!(
        list_output.status.success(),
        "CLI tasks failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );

    let list: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("parse CLI tasks JSON");
    let tasks = list["tasks"].as_array().expect("tasks array");
    assert!(
        tasks.iter().any(|t| t["id"].as_str() == Some(task_id)),
        "task should appear in completed list"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn e2e_lifecycle_all_interfaces_consistent() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let record = server
        .engine
        .enqueue(&queue, E2eTask { data: "consistent".into() })
        .await
        .expect("enqueue");
    let task_id = record.id();

    let client = reqwest::Client::new();
    let rest_body = e2e::wait_for_status(
        &client,
        &server.base_url,
        &task_id.to_string(),
        "completed",
        TIMEOUT,
    )
    .await;

    // Library API
    let lib_record = server
        .engine
        .find(task_id)
        .await
        .expect("find")
        .expect("exists");
    assert_eq!(lib_record.status(), iron_defer::TaskStatus::Completed);
    assert_eq!(lib_record.queue().as_str(), queue.as_str());
    assert_eq!(lib_record.kind().as_ref(), "e2e_test");

    // REST API
    assert_eq!(rest_body["id"], task_id.to_string());
    assert_eq!(rest_body["queue"], queue.as_str());
    assert_eq!(rest_body["kind"], "e2e_test");
    assert_eq!(rest_body["status"], "completed");

    // CLI
    let list_output = assert_cmd::Command::cargo_bin("iron-defer")
        .expect("find binary")
        .env("DATABASE_URL", &server.db_url)
        .args([
            "--json",
            "tasks",
            "--queue",
            &queue,
            "--status",
            "completed",
        ])
        .output()
        .expect("run CLI tasks");
    assert!(list_output.status.success());

    let list: serde_json::Value =
        serde_json::from_slice(&list_output.stdout).expect("parse CLI JSON");
    let cli_task = list["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .find(|t| t["id"].as_str() == Some(&task_id.to_string()))
        .expect("task in CLI output");

    // All three interfaces agree on core fields
    assert_eq!(cli_task["id"].as_str(), rest_body["id"].as_str());
    assert_eq!(cli_task["queue"].as_str(), rest_body["queue"].as_str());
    assert_eq!(cli_task["kind"].as_str(), rest_body["kind"].as_str());
    assert_eq!(cli_task["status"].as_str(), rest_body["status"].as_str());

    server.shutdown().await;
}
