//! Integration tests for idempotency-key submission (Story 9.1).
//!
//! Tests cover:
//! - Duplicate key+queue returns existing task (HTTP 200)
//! - Same key, different queues create separate tasks
//! - No key follows existing behaviour (HTTP 201, no dedup)
//! - Sweeper cleans expired idempotency keys
//! - Expired key allows reuse after cleanup

mod common;

use std::sync::Arc;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdempTask {
    data: String,
}

impl Task for IdempTask {
    const KIND: &'static str = "idemp_test";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

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
            .register::<IdempTask>()
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
// 9.1 — Duplicate key+queue returns same task, HTTP 200
// ---------------------------------------------------------------------------

#[tokio::test]
async fn idempotent_submit_returns_existing_task_on_duplicate() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    let client = reqwest::Client::new();

    let resp1 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "first"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("first submit");

    assert_eq!(resp1.status(), 201, "first submit should be 201 Created");
    let body1: serde_json::Value = resp1.json().await.expect("json");
    let task_id = body1["id"].as_str().expect("id must be string");
    assert_eq!(body1["idempotencyKey"], key);
    assert!(body1["idempotencyExpiresAt"].is_string());

    let resp2 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "duplicate"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("second submit");

    assert_eq!(resp2.status(), 200, "duplicate key should return 200 OK");
    let body2: serde_json::Value = resp2.json().await.expect("json");
    assert_eq!(
        body2["id"].as_str().unwrap(),
        task_id,
        "duplicate must return the original task"
    );
}

// ---------------------------------------------------------------------------
// 9.1 — 10 concurrent retries create exactly 1 task
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_idempotent_submits_create_exactly_one_task() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();
    let barrier = Arc::new(tokio::sync::Barrier::new(10));

    let mut handles = Vec::new();
    for _ in 0..10 {
        let client = reqwest::Client::new();
        let url = server.url("/tasks");
        let q = queue.clone();
        let k = key.clone();
        let b = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            b.wait().await;
            client
                .post(&url)
                .json(&json!({
                    "queue": q,
                    "kind": "idemp_test",
                    "payload": {"data": "concurrent"},
                    "idempotencyKey": k,
                }))
                .send()
                .await
                .expect("send")
        }));
    }

    let mut created_count = 0u32;
    let mut ok_count = 0u32;
    let mut task_ids = std::collections::HashSet::new();

    for h in handles {
        let resp = h.await.expect("join");
        match resp.status().as_u16() {
            201 => created_count += 1,
            200 => ok_count += 1,
            other => panic!("unexpected status {other}"),
        }
        let body: serde_json::Value = resp.json().await.expect("json");
        task_ids.insert(body["id"].as_str().unwrap().to_owned());
    }

    assert_eq!(created_count, 1, "exactly one 201 Created");
    assert_eq!(ok_count, 9, "nine should get 200 OK");
    assert_eq!(task_ids.len(), 1, "all responses reference the same task");

    // Verify DB state: exactly 1 task
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND idempotency_key = $2",
    )
    .bind(&queue)
    .bind(&key)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "exactly 1 task in DB");
}

// ---------------------------------------------------------------------------
// 9.2 — Same key, different queues → two separate tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_key_different_queues_creates_separate_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let key = uuid::Uuid::new_v4().to_string();
    let queue_a = common::unique_queue();
    let queue_b = common::unique_queue();

    let client = reqwest::Client::new();

    let resp_a = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue_a,
            "kind": "idemp_test",
            "payload": {"data": "queue-a"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("queue A");

    assert_eq!(resp_a.status(), 201);
    let body_a: serde_json::Value = resp_a.json().await.expect("json");

    let resp_b = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue_b,
            "kind": "idemp_test",
            "payload": {"data": "queue-b"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("queue B");

    assert_eq!(resp_b.status(), 201, "different queue should create new task");
    let body_b: serde_json::Value = resp_b.json().await.expect("json");

    assert_ne!(
        body_a["id"].as_str().unwrap(),
        body_b["id"].as_str().unwrap(),
        "different queues must produce different task IDs"
    );
}

// ---------------------------------------------------------------------------
// 9.3 — Submit without idempotency key → existing behaviour (201, no dedup)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submit_without_idempotency_key_returns_201() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();

    let client = reqwest::Client::new();

    let resp1 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "no-key-1"},
        }))
        .send()
        .await
        .expect("first");

    assert_eq!(resp1.status(), 201);
    let body1: serde_json::Value = resp1.json().await.expect("json");
    assert!(body1["idempotencyKey"].is_null());

    let resp2 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "no-key-2"},
        }))
        .send()
        .await
        .expect("second");

    assert_eq!(resp2.status(), 201, "no key = always 201");
    let body2: serde_json::Value = resp2.json().await.expect("json");
    assert_ne!(
        body1["id"].as_str().unwrap(),
        body2["id"].as_str().unwrap(),
        "two submissions without key should produce two distinct tasks"
    );
}

// ---------------------------------------------------------------------------
// 9.4 — Sweeper cleans expired idempotency keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sweeper_cleans_expired_idempotency_keys() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    // Insert a completed task with an already-expired idempotency key directly
    let task_id = uuid::Uuid::new_v4();
    let expired_at = chrono::Utc::now() - chrono::Duration::hours(1);
    sqlx::query(
        "INSERT INTO tasks (id, queue, kind, payload, status, priority, attempts, max_attempts, \
         scheduled_at, idempotency_key, idempotency_expires_at) \
         VALUES ($1, $2, 'idemp_test', '{\"data\":\"expired\"}', 'completed', 0, 1, 3, now(), $3, $4)",
    )
    .bind(task_id)
    .bind(&queue)
    .bind(&key)
    .bind(expired_at)
    .execute(&pool)
    .await
    .expect("insert expired task");

    // Verify the key is present before cleanup
    let (pre_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE id = $1 AND idempotency_key IS NOT NULL",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .expect("pre-count");
    assert_eq!(pre_count, 1, "key should be present before cleanup");

    // Run the sweeper cleanup directly via the repository
    let repo = iron_defer_infrastructure::PostgresTaskRepository::new(pool.clone(), false);
    use iron_defer_application::ports::TaskRepository;
    let cleaned = repo.cleanup_expired_idempotency_keys().await.expect("cleanup");
    assert!(cleaned >= 1, "should clean at least 1 expired key");

    // Verify key is now NULL
    let (post_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE id = $1 AND idempotency_key IS NOT NULL",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .expect("post-count");
    assert_eq!(post_count, 0, "key should be NULLed after cleanup");

    // Verify the task record still exists (not deleted)
    let (exists,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks WHERE id = $1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .expect("exists");
    assert_eq!(exists, 1, "task record must still exist after key cleanup");
}

// ---------------------------------------------------------------------------
// 9.5 — Expired key allows reuse after cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expired_key_allows_reuse_after_cleanup() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let server = TestServer::start(&pool).await;
    let queue = common::unique_queue();
    let key = uuid::Uuid::new_v4().to_string();

    let client = reqwest::Client::new();

    // Submit the first task with the key
    let resp1 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "original"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("first submit");
    assert_eq!(resp1.status(), 201);
    let body1: serde_json::Value = resp1.json().await.expect("json");
    let original_id = body1["id"].as_str().unwrap().to_owned();

    // Simulate terminal state + expired retention: mark the task completed
    // and set its expiry in the past.
    sqlx::query(
        "UPDATE tasks SET status = 'completed', idempotency_expires_at = now() - interval '1 hour' \
         WHERE id = $1::uuid",
    )
    .bind(&original_id)
    .execute(&pool)
    .await
    .expect("mark completed + expired");

    // Run sweeper cleanup to NULL the key
    let repo = iron_defer_infrastructure::PostgresTaskRepository::new(pool.clone(), false);
    use iron_defer_application::ports::TaskRepository;
    repo.cleanup_expired_idempotency_keys().await.expect("cleanup");

    // Submit again with the same key — should create a NEW task (201)
    let resp2 = client
        .post(server.url("/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "idemp_test",
            "payload": {"data": "reused"},
            "idempotencyKey": key,
        }))
        .send()
        .await
        .expect("reuse submit");
    assert_eq!(resp2.status(), 201, "after cleanup the key should be reusable");
    let body2: serde_json::Value = resp2.json().await.expect("json");
    assert_ne!(
        body2["id"].as_str().unwrap(),
        original_id,
        "reuse must create a different task"
    );
}