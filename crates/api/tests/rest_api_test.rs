//! REST API integration tests (Story 1B.3, AC 8).
//!
//! Each test gets its own pool on the shared testcontainers Postgres
//! instance via `common::fresh_pool_on_shared_container()`. Each test
//! scopes its writes with `common::unique_queue()` so concurrent tests
//! do not collide.

mod common;

use std::sync::Arc;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ---------------------------------------------------------------------------
// Test task type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestTestTask {
    data: String,
}

impl Task for RestTestTask {
    const KIND: &'static str = "rest_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowTask {
    sleep_ms: u64,
}

impl Task for SlowTask {
    const KIND: &'static str = "slow_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper: build engine + start HTTP server on random port
// ---------------------------------------------------------------------------

struct TestServer {
    base_url: String,
    _engine: Arc<IronDefer>,
    token: CancellationToken,
    _handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(pool: &sqlx::PgPool) -> Self {
        let engine = IronDefer::builder()
            .pool(pool.clone())
            .register::<RestTestTask>()
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine");

        let engine = Arc::new(engine);
        let token = CancellationToken::new();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let base_url = format!("http://127.0.0.1:{}", addr.port());

        let router = iron_defer::http::router::build(Arc::clone(&engine));
        let server_token = token.clone();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(server_token.cancelled_owned())
                .await
                .expect("server");
        });

        Self {
            base_url,
            _engine: engine,
            token,
            _handle: handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// POST a task, verify HTTP 201 + camelCase JSON fields.
#[tokio::test]
async fn post_task_returns_201_with_camel_case_body() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "hello"}
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 201, "expected HTTP 201 Created");

    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(body["id"].is_string(), "id should be a string UUID");
    assert_eq!(body["queue"], queue);
    assert_eq!(body["kind"], "rest_test");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["priority"], 0);
    assert_eq!(body["attempts"], 0);
    assert_eq!(body["maxAttempts"], 3);
    // Verify camelCase fields exist
    assert!(body["scheduledAt"].is_string(), "scheduledAt should exist");
    assert!(body["createdAt"].is_string(), "createdAt should exist");
    assert!(body["updatedAt"].is_string(), "updatedAt should exist");
    // Verify snake_case fields do NOT exist
    assert!(
        body["scheduled_at"].is_null(),
        "snake_case should not appear"
    );
    assert!(body["created_at"].is_null(), "snake_case should not appear");
}

/// POST with optional scheduledAt and priority, verify they are stored.
#[tokio::test]
async fn post_task_with_scheduled_at_and_priority() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "scheduled"},
            "scheduledAt": "2030-01-01T00:00:00Z",
            "priority": 5,
            "maxAttempts": 10
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["priority"], 5);
    assert_eq!(body["maxAttempts"], 10);
    assert!(
        body["scheduledAt"]
            .as_str()
            .unwrap_or("")
            .starts_with("2030"),
        "scheduledAt should be in 2030"
    );
}

/// POST without `kind` returns HTTP 422 + error body.
#[tokio::test]
async fn post_task_missing_kind_returns_422() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "payload": {"data": "no-kind"}
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 422, "missing kind should yield 422");

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "INVALID_PAYLOAD");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("missing field"),
        "422 message should mention the missing field, got: {:?}",
        body["error"]["message"]
    );
}

/// POST with unregistered `kind` returns HTTP 422.
#[tokio::test]
async fn post_task_unknown_kind_returns_422() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "kind": "nonexistent_handler",
            "payload": {}
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 422, "unknown kind should yield 422");

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "INVALID_PAYLOAD");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("no handler registered"),
        "message should mention missing handler"
    );
}

/// POST then GET — verify round-trip.
#[tokio::test]
async fn get_task_returns_200() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    // POST
    let post_resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "round-trip"}
        }))
        .send()
        .await
        .expect("send");
    assert_eq!(post_resp.status(), 201);
    let post_body: serde_json::Value = post_resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    // GET
    let get_resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), 200);
    let get_body: serde_json::Value = get_resp.json().await.expect("json");

    assert_eq!(get_body["id"], task_id);
    assert_eq!(get_body["queue"], queue);
    assert_eq!(get_body["kind"], "rest_test");
    assert_eq!(get_body["status"], "pending");
}

/// GET a random UUID returns HTTP 404 + error body.
#[tokio::test]
async fn get_nonexistent_task_returns_404() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{fake_id}")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_NOT_FOUND");
}

/// P2-API-001 — error responses from internal failures must not leak stack
/// traces, SQL fragments, or other implementation details to the client.
#[tokio::test]
async fn error_responses_do_not_leak_internals() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    // 1. 422 from missing kind — structured JSON error, no Rust internals
    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({"payload": {}}))
        .send()
        .await
        .expect("send");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        body["error"]["code"].is_string(),
        "422 response must have structured error.code"
    );
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        !msg.contains("panic"),
        "422 message must not contain 'panic': {msg}"
    );
    assert!(
        !msg.contains("stack backtrace"),
        "422 message must not contain stack traces: {msg}"
    );

    // 2. 422 from unknown kind — structured error, no internals
    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({"kind": "nonexistent", "payload": {}}))
        .send()
        .await
        .expect("send");
    let body: serde_json::Value = resp.json().await.expect("json");
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        !msg.contains("src/"),
        "error message must not contain source file paths: {msg}"
    );
    assert!(
        !msg.contains("thread '"),
        "error message must not contain thread names: {msg}"
    );

    // 3. 404 from missing task — no SQL table names
    let fake_id = uuid::Uuid::new_v4();
    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{fake_id}")))
        .send()
        .await
        .expect("send");
    let body: serde_json::Value = resp.json().await.expect("json");
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        !msg.contains("SELECT"),
        "404 message must not contain SQL: {msg}"
    );
    assert!(
        !msg.contains("tasks"),
        "404 message must not expose table names: {msg}"
    );
}

/// P2-API-002 — common admin/debug paths must not be exposed. Only the
/// three registered routes (/tasks, /tasks/{id}, /metrics) should respond.
#[tokio::test]
async fn no_hidden_admin_or_debug_endpoints() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let suspicious_paths = [
        "/admin",
        "/debug",
        "/debug/vars",
        "/debug/pprof",
        "/healthz",
        "/readiness",
        "/api/v1/tasks",
        "/swagger.json",
        "/.env",
        "/config",
    ];

    let client = reqwest::Client::new();
    for path in suspicious_paths {
        let resp = client.get(server.url(path)).send().await.expect("send");
        assert!(
            resp.status() == 404 || resp.status() == 405,
            "path {path} returned {} — expected 404 or 405",
            resp.status()
        );
    }
}

// ---------------------------------------------------------------------------
// Health probe tests (Story 4.1 AC 1, AC 2)
// ---------------------------------------------------------------------------

/// GET /health returns 200 with empty JSON object.
#[tokio::test]
async fn health_liveness_returns_200() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/health"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body,
        json!({}),
        "liveness probe should return empty JSON object"
    );
}

/// GET /health/ready returns 200 when DB is connected.
#[tokio::test]
async fn health_readiness_returns_200_when_db_connected() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/health/ready"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "ready");
    assert_eq!(body["db"], "ok");
}

// ---------------------------------------------------------------------------
// Cancel tests (Story 4.1 AC 3, AC 4)
// ---------------------------------------------------------------------------

/// Cancel a pending task — returns 200 with cancelled status.
#[tokio::test]
async fn cancel_pending_task_returns_200() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    // Create a task
    let post_resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "cancel-me"}
        }))
        .send()
        .await
        .expect("send");
    assert_eq!(post_resp.status(), 201);
    let post_body: serde_json::Value = post_resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    // Cancel
    let del_resp = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(
        del_resp.status(),
        200,
        "cancel pending task should return 200"
    );

    let del_body: serde_json::Value = del_resp.json().await.expect("json");
    assert_eq!(del_body["id"], task_id);
    assert_eq!(del_body["status"], "cancelled");

    // Verify via GET that the task is now cancelled
    let get_resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), 200);
    let get_body: serde_json::Value = get_resp.json().await.expect("json");
    assert_eq!(get_body["status"], "cancelled");
}

/// Cancel a suspended task — returns 200 with cancelled status.
#[tokio::test]
async fn cancel_suspended_task_returns_200() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    // Create a task
    let post_resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&serde_json::json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "cancel-me-suspended"}
        }))
        .send()
        .await
        .expect("send");
    assert_eq!(post_resp.status(), 201);
    let post_body: serde_json::Value = post_resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    // Force transition to suspended via DB
    sqlx::query(
        "UPDATE tasks SET status = 'suspended' WHERE id = $1"
    )
    .bind(uuid::Uuid::parse_str(task_id).unwrap())
    .execute(pool)
    .await
    .expect("update status to suspended");

    // Cancel
    let del_resp = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(
        del_resp.status(),
        200,
        "cancel suspended task should return 200"
    );

    let del_body: serde_json::Value = del_resp.json().await.expect("json");
    assert_eq!(del_body["id"], task_id);
    assert_eq!(del_body["status"], "cancelled");

    // Verify via GET that the task is now cancelled
    let get_resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), 200);
    let get_body: serde_json::Value = get_resp.json().await.expect("json");
    assert_eq!(get_body["status"], "cancelled");
}

/// Cancel a non-existent task — returns 404.
#[tokio::test]
async fn cancel_nonexistent_task_returns_404() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    let fake_id = uuid::Uuid::new_v4();
    let resp = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{fake_id}")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_NOT_FOUND");
}

/// Cancel an already-cancelled task — returns 409.
#[tokio::test]
async fn cancel_already_cancelled_task_returns_409() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    // Create and cancel a task
    let post_resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "double-cancel"}
        }))
        .send()
        .await
        .expect("send");
    let post_body: serde_json::Value = post_resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id");

    // First cancel
    let resp1 = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), 200);

    // Second cancel
    let resp2 = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), 409);
    let body: serde_json::Value = resp2.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_IN_TERMINAL_STATE");
}

/// Story 6.2 AC 5 — 10 concurrent DELETE /tasks/{id} for the same pending
/// task: exactly one receives 200, the rest 409 (or 404), final status is
/// cancelled with no data corruption.
#[tokio::test]
async fn concurrent_cancel_exactly_one_succeeds() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let post_resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "rest_test",
            "payload": {"data": "concurrent-cancel"}
        }))
        .send()
        .await
        .expect("send");
    assert_eq!(post_resp.status(), 201);
    let post_body: serde_json::Value = post_resp.json().await.expect("json");
    let task_id = post_body["id"].as_str().expect("id").to_owned();

    let client = reqwest::Client::new();
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let c = client.clone();
        let url = server.url(&format!("/tasks/{task_id}"));
        set.spawn(async move { c.delete(url).send().await.expect("send").status() });
    }

    let mut statuses = Vec::with_capacity(10);
    while let Some(result) = set.join_next().await {
        statuses.push(result.expect("join"));
    }

    let ok_count = statuses
        .iter()
        .filter(|s| **s == reqwest::StatusCode::OK)
        .count();
    let conflict_or_not_found = statuses
        .iter()
        .filter(|s| **s == reqwest::StatusCode::CONFLICT || **s == reqwest::StatusCode::NOT_FOUND)
        .count();

    assert_eq!(
        ok_count, 1,
        "exactly one cancel should succeed with 200, got {ok_count}; all statuses: {statuses:?}"
    );
    assert_eq!(
        conflict_or_not_found, 9,
        "remaining 9 should be 409 or 404, got {conflict_or_not_found}; all statuses: {statuses:?}"
    );

    let get_resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks/{task_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), 200);
    let get_body: serde_json::Value = get_resp.json().await.expect("json");
    assert_eq!(
        get_body["status"], "cancelled",
        "final task status must be cancelled"
    );
}

/// Cancel a running task — returns 409 with `TASK_ALREADY_CLAIMED`.
#[tokio::test]
async fn cancel_running_task_returns_409() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    // Build engine with a slow task to keep it in Running state
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<RestTestTask>()
        .register::<SlowTask>()
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    // Start worker so it claims tasks
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let _worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let _server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    // Enqueue a slow task
    let record = engine
        .enqueue(&queue, SlowTask { sleep_ms: 5000 })
        .await
        .expect("enqueue");
    let task_id = record.id();

    // Wait for the worker to claim it
    let mut claimed = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Some(r) = engine.find(task_id).await.expect("find") {
            if r.status() == iron_defer::TaskStatus::Running {
                claimed = true;
                break;
            }
        }
    }
    assert!(claimed, "task should be claimed within 6 seconds");

    // Try to cancel the running task
    let resp = reqwest::Client::new()
        .delete(format!("{base_url}/tasks/{}", task_id.as_uuid()))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 409);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_ALREADY_CLAIMED");

    token.cancel();
}

/// Cancel a completed task — returns 409 with `TASK_IN_TERMINAL_STATE`.
#[tokio::test]
async fn cancel_completed_task_returns_409() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<RestTestTask>()
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);
    let token = CancellationToken::new();

    // Start worker
    let engine_ref = Arc::clone(&engine);
    let worker_token = token.clone();
    let _worker_handle = tokio::spawn(async move {
        let _ = engine_ref.start(worker_token).await;
    });

    // Enqueue a fast task
    let record = engine
        .enqueue(
            &queue,
            RestTestTask {
                data: "complete-then-cancel".into(),
            },
        )
        .await
        .expect("enqueue");
    let task_id = record.id();

    // Wait for completion
    let mut completed = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Some(r) = engine.find(task_id).await.expect("find") {
            if r.status() == iron_defer::TaskStatus::Completed {
                completed = true;
                break;
            }
        }
    }
    assert!(completed, "task should complete within 6 seconds");

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let server_token = token.clone();
    let _server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    // Try to cancel
    let resp = reqwest::Client::new()
        .delete(format!("{base_url}/tasks/{}", task_id.as_uuid()))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 409);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "TASK_IN_TERMINAL_STATE");

    token.cancel();
}

/// **LOAD-BEARING TEST** — POST a body exceeding 1 MiB, verify rejection.
#[tokio::test]
async fn post_task_body_limit_enforced() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;

    // Build a payload slightly over 1 MiB
    let big_string = "x".repeat(1_100_000);
    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "kind": "rest_test",
            "payload": {"data": big_string}
        }))
        .send()
        .await
        .expect("send");

    // axum returns 413 Payload Too Large when DefaultBodyLimit is exceeded
    assert_eq!(
        resp.status(),
        413,
        "body over 1 MiB should be rejected with 413"
    );
}

/// P3-INT-004 — POST a payload just under 1 MiB, verify acceptance (201).
/// Symmetric boundary test for `post_task_body_limit_enforced`.
#[tokio::test]
async fn post_task_near_1mib_payload_accepted() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let server = TestServer::start(pool).await;
    let queue = common::unique_queue();

    // JSON envelope overhead: {"kind":"rest_test","queue":"<uuid>","payload":{"data":"..."}}
    // is ~80-120 bytes. Use 900_000 bytes of payload data to stay well under
    // the 1,048,576-byte limit while still testing a large payload path.
    let large_string = "x".repeat(900_000);
    let resp = reqwest::Client::new()
        .post(server.url("/tasks"))
        .json(&json!({
            "kind": "rest_test",
            "queue": queue,
            "payload": {"data": large_string}
        }))
        .send()
        .await
        .expect("send");

    assert_eq!(
        resp.status(),
        201,
        "payload under 1 MiB should be accepted with 201"
    );
}

// ---------------------------------------------------------------------------
// Story 4.2 — List tasks (GET /tasks)
// ---------------------------------------------------------------------------

/// Helper: create N tasks in the given queue via POST /tasks.
async fn create_tasks(server: &TestServer, queue: &str, count: usize) -> Vec<serde_json::Value> {
    let client = reqwest::Client::new();
    let mut tasks = Vec::with_capacity(count);
    for i in 0..count {
        let resp = client
            .post(server.url("/tasks"))
            .json(&json!({
                "queue": queue,
                "kind": "rest_test",
                "payload": {"index": i}
            }))
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 201);
        tasks.push(resp.json::<serde_json::Value>().await.expect("json"));
    }
    tasks
}

#[tokio::test]
async fn list_tasks_returns_all_when_no_filters() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    create_tasks(&server, &queue, 3).await;

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 3);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 3);
    assert_eq!(body["limit"], 50);
    assert_eq!(body["offset"], 0);
}

#[tokio::test]
async fn list_tasks_filters_by_queue() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue_a = common::unique_queue();
    let queue_b = common::unique_queue();

    create_tasks(&server, &queue_a, 2).await;
    create_tasks(&server, &queue_b, 3).await;

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue_a}")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 2);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 2);

    for task in body["tasks"].as_array().unwrap() {
        assert_eq!(task["queue"], queue_a.as_str());
    }
}

#[tokio::test]
async fn list_tasks_filters_by_status() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let tasks = create_tasks(&server, &queue, 3).await;
    let cancel_id = tasks[0]["id"].as_str().unwrap();

    let resp = reqwest::Client::new()
        .delete(server.url(&format!("/tasks/{cancel_id}")))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}&status=pending")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 2);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn list_tasks_pagination_limit_offset() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    create_tasks(&server, &queue, 5).await;

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}&limit=2&offset=2")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 5);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(body["limit"], 2);
    assert_eq!(body["offset"], 2);
}

#[tokio::test]
async fn list_tasks_limit_clamped_to_max() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    create_tasks(&server, &queue, 2).await;

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}&limit=500")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["limit"], 100, "limit should be clamped to max 100");
}

#[tokio::test]
async fn list_tasks_invalid_status_returns_422() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/tasks?status=bogus"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 422);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "INVALID_QUERY_PARAMETER");
}

#[tokio::test]
async fn list_tasks_empty_result() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 0);
    assert!(body["tasks"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Story 7.1 — Offset capping (AC1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_tasks_offset_capped_at_10000() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    create_tasks(&server, &queue, 1).await;

    let resp = reqwest::Client::new()
        .get(server.url(&format!("/tasks?queue={queue}&offset=20000")))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["offset"], 10000, "offset should be capped at 10000");
}

// ---------------------------------------------------------------------------
// Story 7.1 — Unfiltered query warning (AC2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_tasks_fails_unfiltered() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/tasks"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 422);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(body["error"]["message"].as_str().unwrap().contains("please provide a 'queue' or 'status' filter"));
}

#[tokio::test]
async fn list_tasks_with_explicit_limit() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/tasks?queue=default&limit=10"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body["limit"], 10,
        "query with explicit limit=10 should use 10"
    );
}

// ---------------------------------------------------------------------------
// Story 7.1 — Case-insensitive status (AC5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_tasks_case_insensitive_status_filter() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    create_tasks(&server, &queue, 2).await;

    for variant in ["PENDING", "Pending", "pEnDiNg"] {
        let resp = reqwest::Client::new()
            .get(server.url(&format!("/tasks?queue={queue}&status={variant}")))
            .send()
            .await
            .expect("send");

        assert_eq!(resp.status(), 200, "status={variant} should be accepted");
        let body: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(
            body["total"], 2,
            "status={variant} should match 2 pending tasks"
        );
    }
}

// ---------------------------------------------------------------------------
// Story 7.1 — Queue stats active-only (AC4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queue_stats_excludes_terminal_only_queues() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let tasks = create_tasks(&server, &queue, 2).await;

    // Cancel both tasks so all are in terminal state
    for task in &tasks {
        let id = task["id"].as_str().unwrap();
        let resp = reqwest::Client::new()
            .delete(server.url(&format!("/tasks/{id}")))
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 200);
    }

    let resp = reqwest::Client::new()
        .get(server.url("/queues"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert!(
        !body.iter().any(|q| q["queue"] == queue.as_str()),
        "queue with only cancelled tasks should not appear in stats"
    );
}

// ---------------------------------------------------------------------------
// Story 4.2 — Queue stats (GET /queues)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn queue_stats_returns_queue_list() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue_a = common::unique_queue();
    let queue_b = common::unique_queue();

    create_tasks(&server, &queue_a, 2).await;
    create_tasks(&server, &queue_b, 3).await;

    let resp = reqwest::Client::new()
        .get(server.url("/queues"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.expect("json");

    let find_queue = |name: &str| body.iter().find(|q| q["queue"] == name);

    let qa = find_queue(&queue_a).expect("queue_a in stats");
    assert_eq!(qa["pending"], 2);

    let qb = find_queue(&queue_b).expect("queue_b in stats");
    assert_eq!(qb["pending"], 3);
}

#[tokio::test]
async fn queue_stats_empty_when_no_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let resp = reqwest::Client::new()
        .get(server.url("/queues"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert!(
        !body.iter().any(|q| q["queue"] == queue.as_str()),
        "queue with no tasks should not appear"
    );
}

// ---------------------------------------------------------------------------
// Story 4.2 — OpenAPI spec (GET /openapi.json)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openapi_spec_returns_valid_json() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/openapi.json"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["openapi"], "3.1.0");
    assert!(body["info"]["title"].is_string());
}

#[tokio::test]
async fn openapi_spec_documents_all_endpoints() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        return;
    };
    let server = TestServer::start(&pool).await;

    let resp = reqwest::Client::new()
        .get(server.url("/openapi.json"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let paths = body["paths"].as_object().expect("paths object");

    let expected_paths = [
        "/tasks",
        "/tasks/{id}",
        "/queues",
        "/health",
        "/health/ready",
        "/metrics",
        "/openapi.json",
    ];
    for path in expected_paths {
        assert!(
            paths.contains_key(path),
            "missing path {path} in OpenAPI spec"
        );
    }
}
