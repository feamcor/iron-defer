//! E2E CLI-to-REST consistency test (Story 8.2, AC 5).
//!
//! Submits a task via CLI, queries via REST, and verifies field-level
//! consistency between the two interfaces.

mod common;

use common::e2e;

#[tokio::test]
async fn e2e_cli_to_rest_field_consistency() {
    let worker_queue = common::unique_queue();
    let test_queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&worker_queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Submit to a queue without workers so status stays pending
    let output = assert_cmd::Command::cargo_bin("iron-defer")
        .expect("find binary")
        .env("DATABASE_URL", &server.db_url)
        .args([
            "--json",
            "submit",
            "--queue",
            &test_queue,
            "--kind",
            "e2e_test",
            "--payload",
            r#"{"data":"consistency"}"#,
            "--priority",
            "7",
        ])
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI submit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cli_task: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse CLI JSON");
    let task_id = cli_task["id"].as_str().expect("CLI task id");

    // Query same task via REST
    let client = reqwest::Client::new();
    let resp = client
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    let rest_task: serde_json::Value = resp.json().await.expect("json");

    // Field-level consistency: id, queue, kind, status, priority
    assert_eq!(
        cli_task["id"].as_str(),
        rest_task["id"].as_str(),
        "id mismatch"
    );
    assert_eq!(
        cli_task["queue"].as_str(),
        rest_task["queue"].as_str(),
        "queue mismatch"
    );
    assert_eq!(
        cli_task["kind"].as_str(),
        rest_task["kind"].as_str(),
        "kind mismatch"
    );
    assert_eq!(
        cli_task["status"].as_str(),
        rest_task["status"].as_str(),
        "status mismatch"
    );
    assert_eq!(
        cli_task["priority"].as_i64(),
        rest_task["priority"].as_i64(),
        "priority mismatch"
    );

    server.shutdown().await;
}

/// Verify CLI tasks list also matches REST for the same task.
#[tokio::test]
async fn e2e_cli_list_matches_rest() {
    let worker_queue = common::unique_queue();
    let test_queue = common::unique_queue(); // unique queue, no workers
    let Some((server, _pool)) = e2e::boot_e2e_engine(&worker_queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    // Submit to a queue without workers so status stays pending
    let client = reqwest::Client::new();
    let resp = client
        .post(server.url("/tasks"))
        .json(&serde_json::json!({
            "queue": test_queue,
            "kind": "e2e_test",
            "payload": {"data": "list-check"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 201);
    let rest_created: serde_json::Value = resp.json().await.expect("json");
    let task_id = rest_created["id"].as_str().expect("id");

    // Query via REST list
    let resp = client
        .get(server.url(&format!("/tasks?queue={test_queue}")))
        .send()
        .await
        .expect("list");
    let rest_list: serde_json::Value = resp.json().await.expect("json");
    let rest_task = rest_list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(task_id))
        .expect("task in REST list");

    // Query via CLI tasks list
    let output = assert_cmd::Command::cargo_bin("iron-defer")
        .expect("find binary")
        .env("DATABASE_URL", &server.db_url)
        .args(["--json", "tasks", "--queue", &test_queue])
        .output()
        .expect("run CLI tasks");
    assert!(output.status.success());

    let cli_list: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse CLI JSON");
    let cli_task = cli_list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"].as_str() == Some(task_id))
        .expect("task in CLI list");

    // Field-level consistency
    assert_eq!(cli_task["id"].as_str(), rest_task["id"].as_str());
    assert_eq!(cli_task["queue"].as_str(), rest_task["queue"].as_str());
    assert_eq!(cli_task["kind"].as_str(), rest_task["kind"].as_str());
    assert_eq!(cli_task["status"].as_str(), rest_task["status"].as_str());
    assert_eq!(cli_task["priority"].as_i64(), rest_task["priority"].as_i64());

    server.shutdown().await;
}
