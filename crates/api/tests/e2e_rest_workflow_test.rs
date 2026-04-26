//! E2E REST workflow and error-path tests (Story 8.2, AC 3 & AC 4).
//!
//! Tests multi-step sequenced operations through the REST API rather than
//! re-testing individual endpoints already covered by `rest_api_test.rs`.

mod common;

use common::e2e;
use serde_json::json;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// AC3: POST → GET → list → DELETE → GET → list(excluded) workflow.
#[tokio::test]
async fn e2e_rest_create_read_list_cancel_workflow() {
    let worker_queue = common::unique_queue();
    let test_queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&worker_queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();

    // Step 1: POST /tasks (create)
    let resp = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": test_queue,
            "kind": "e2e_test",
            "payload": {"data": "workflow"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().await.expect("json");
    let task_id = created["id"].as_str().expect("id");
    assert_eq!(created["status"], "pending");

    // Step 2: GET /tasks/{id} (verify created)
    let resp = client
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    let fetched: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(fetched["id"], task_id);
    assert_eq!(fetched["status"], "pending");
    assert_eq!(fetched["queue"], test_queue.as_str());

    // Step 3: GET /tasks?queue=X&status=pending (verify in list)
    let resp = client
        .get(server.url(&format!("/tasks?queue={test_queue}&status=pending")))
        .send()
        .await
        .expect("list");
    assert_eq!(resp.status(), 200);
    let list: serde_json::Value = resp.json().await.expect("json");
    assert!(
        list["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"].as_str() == Some(task_id)),
        "task should appear in pending list"
    );

    // Step 4: DELETE /tasks/{id} (cancel)
    let resp = client
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("delete");
    assert_eq!(resp.status(), 200);
    let cancelled: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(cancelled["status"], "cancelled");

    // Step 5: GET /tasks/{id} (verify cancelled)
    let resp = client
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    let verified: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(verified["status"], "cancelled");

    // Step 6: cancelled task excluded from pending list
    let resp = client
        .get(server.url(&format!("/tasks?queue={test_queue}&status=pending")))
        .send()
        .await
        .expect("list");
    assert_eq!(resp.status(), 200);
    let list: serde_json::Value = resp.json().await.expect("json");
    assert!(
        !list["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"].as_str() == Some(task_id)),
        "cancelled task should not appear in pending list"
    );

    server.shutdown().await;
}

/// AC4: Error-path E2E — a workflow that encounters all three error conditions.
#[tokio::test]
async fn e2e_error_path_workflow() {
    let queue = common::unique_queue();
    let Some((server, _pool)) = e2e::boot_e2e_engine(&queue).await else {
        eprintln!("[skip] Docker not available");
        return;
    };

    let client = reqwest::Client::new();

    // Error 1: POST /tasks with missing kind → 422 INVALID_PAYLOAD
    let resp = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "payload": {"data": "no-kind"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 422);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "INVALID_PAYLOAD");

    // Error 2: GET /tasks/{non-existent} → 404 TASK_NOT_FOUND
    let fake_id = uuid::Uuid::new_v4();
    let resp = client
        .get(server.url(&format!("/tasks/{fake_id}")))
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_NOT_FOUND");

    // Submit a task and wait for it to complete
    let resp = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "e2e_test",
            "payload": {"data": "complete-then-cancel"}
        }))
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().await.expect("json");
    let task_id = created["id"].as_str().expect("id");

    e2e::wait_for_status(&client, &server.base_url, task_id, "completed", TIMEOUT).await;

    // Error 3: DELETE /tasks/{completed} → 409 TASK_IN_TERMINAL_STATE
    let resp = client
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("delete");
    assert_eq!(resp.status(), 409);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_IN_TERMINAL_STATE");

    server.shutdown().await;
}
