mod common;

use common::e2e::E2eTask;
use iron_defer::{
    CancellationToken, IronDefer, Task, TaskContext, TaskError, TaskStatus, WorkerConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegionE2eTask {}

impl Task for RegionE2eTask {
    const KIND: &'static str = "region_e2e";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

async fn build_regional_engine(
    pool: &sqlx::PgPool,
    queue: &str,
    region: Option<&str>,
) -> IronDefer {
    IronDefer::builder()
        .pool(pool.clone())
        .register::<RegionE2eTask>()
        .register::<E2eTask>()
        .worker_config(WorkerConfig {
            concurrency: 1,
            poll_interval: Duration::from_millis(50),
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            shutdown_timeout: Duration::from_secs(2),
            region: region.map(str::to_owned),
            ..WorkerConfig::default()
        })
        .queue(queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build regional engine")
}

#[tokio::test]
async fn e2e_pinned_task_correct_region() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let queue = common::unique_queue();

    let engine_eu = build_regional_engine(&pool, &queue, Some("eu-west")).await;
    let engine_us = build_regional_engine(&pool, &queue, Some("us-east")).await;

    // Submit pinned to eu-west
    let record = engine_eu
        .enqueue_with_region::<RegionE2eTask>(&queue, RegionE2eTask {}, "eu-west")
        .await
        .expect("enqueue");
    let task_id = record.id();

    let eu_token = CancellationToken::new();
    let us_token = CancellationToken::new();
    let eu_cancel = eu_token.clone();
    let us_cancel = us_token.clone();

    let engine_eu = Arc::new(engine_eu);
    let engine_us = Arc::new(engine_us);

    let eu_ref = Arc::clone(&engine_eu);
    let eu_tok = eu_token.clone();
    let eu_handle = tokio::spawn(async move { eu_ref.start(eu_tok).await });

    let us_ref = Arc::clone(&engine_us);
    let us_tok = us_token.clone();
    let us_handle = tokio::spawn(async move { us_ref.start(us_tok).await });

    // Wait for completion
    let mut completed = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(r) = engine_eu.find(task_id).await.unwrap() {
            if r.status() == TaskStatus::Completed {
                completed = true;
                break;
            }
        }
    }

    eu_cancel.cancel();
    us_cancel.cancel();
    let _ = eu_handle.await;
    let _ = us_handle.await;

    assert!(completed, "eu-west task should complete");

    let final_task = engine_eu.find(task_id).await.unwrap().unwrap();
    assert_eq!(final_task.status(), TaskStatus::Completed);
}

#[tokio::test]
async fn e2e_unpinned_claimed_by_any() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let queue = common::unique_queue();

    let engine_eu = build_regional_engine(&pool, &queue, Some("eu-west")).await;
    let engine_us = build_regional_engine(&pool, &queue, Some("us-east")).await;

    // Submit unpinned task
    let record = engine_eu
        .enqueue::<RegionE2eTask>(&queue, RegionE2eTask {})
        .await
        .expect("enqueue");
    let task_id = record.id();

    let eu_token = CancellationToken::new();
    let us_token = CancellationToken::new();
    let eu_cancel = eu_token.clone();
    let us_cancel = us_token.clone();

    let engine_eu = Arc::new(engine_eu);
    let engine_us = Arc::new(engine_us);

    let eu_ref = Arc::clone(&engine_eu);
    let eu_tok = eu_token.clone();
    let eu_handle = tokio::spawn(async move { eu_ref.start(eu_tok).await });

    let us_ref = Arc::clone(&engine_us);
    let us_tok = us_token.clone();
    let us_handle = tokio::spawn(async move { us_ref.start(us_tok).await });

    let mut completed = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(r) = engine_eu.find(task_id).await.unwrap() {
            if r.status() == TaskStatus::Completed {
                completed = true;
                break;
            }
        }
    }

    eu_cancel.cancel();
    us_cancel.cancel();
    let _ = eu_handle.await;
    let _ = us_handle.await;

    assert!(completed, "unpinned task should be claimed by any regional worker");
}

#[tokio::test]
async fn e2e_regionless_worker_skips_pinned() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let queue = common::unique_queue();

    let engine_none = build_regional_engine(&pool, &queue, None).await;

    // Submit pinned task
    let record = engine_none
        .enqueue_with_region::<RegionE2eTask>(&queue, RegionE2eTask {}, "eu-west")
        .await
        .expect("enqueue");
    let task_id = record.id();

    let token = CancellationToken::new();
    let cancel = token.clone();
    let engine_none = Arc::new(engine_none);
    let eng_ref = Arc::clone(&engine_none);
    let tok = token.clone();
    let handle = tokio::spawn(async move { eng_ref.start(tok).await });

    // Poll multiple times — task should remain Pending (regionless worker skips pinned tasks)
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let r = engine_none.find(task_id).await.unwrap().unwrap();
        assert_eq!(r.status(), TaskStatus::Pending, "regionless worker should skip pinned task");
    }

    // Now start a regional worker to claim it
    cancel.cancel();
    let _ = handle.await;

    let engine_eu = build_regional_engine(&pool, &queue, Some("eu-west")).await;
    let token2 = CancellationToken::new();
    let cancel2 = token2.clone();
    let engine_eu = Arc::new(engine_eu);
    let eu_ref = Arc::clone(&engine_eu);
    let tok2 = token2.clone();
    let handle2 = tokio::spawn(async move { eu_ref.start(tok2).await });

    let mut completed = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(r) = engine_eu.find(task_id).await.unwrap() {
            if r.status() == TaskStatus::Completed {
                completed = true;
                break;
            }
        }
    }
    cancel2.cancel();
    let _ = handle2.await;

    assert!(completed, "eu-west worker should claim the pinned task");
}

#[tokio::test]
async fn e2e_regional_worker_claims_both() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let queue = common::unique_queue();

    let engine = build_regional_engine(&pool, &queue, Some("eu-west")).await;

    let pinned = engine
        .enqueue_with_region::<RegionE2eTask>(&queue, RegionE2eTask {}, "eu-west")
        .await
        .expect("enqueue pinned");
    let unpinned = engine
        .enqueue::<RegionE2eTask>(&queue, RegionE2eTask {})
        .await
        .expect("enqueue unpinned");

    let pinned_id = pinned.id();
    let unpinned_id = unpinned.id();

    let token = CancellationToken::new();
    let cancel = token.clone();
    let engine = Arc::new(engine);
    let eng_ref = Arc::clone(&engine);
    let tok = token.clone();
    let handle = tokio::spawn(async move { eng_ref.start(tok).await });

    let mut both_done = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let p = engine.find(pinned_id).await.unwrap();
        let u = engine.find(unpinned_id).await.unwrap();
        if p.map(|r| r.status()) == Some(TaskStatus::Completed)
            && u.map(|r| r.status()) == Some(TaskStatus::Completed)
        {
            both_done = true;
            break;
        }
    }
    cancel.cancel();
    let _ = handle.await;
    assert!(both_done, "eu-west worker should claim both pinned eu-west and unpinned tasks");
}

#[tokio::test]
async fn e2e_region_visible_in_rest() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker unavailable");
        return;
    };
    let queue = common::unique_queue();

    let engine = build_regional_engine(&pool, &queue, None).await;
    let engine = Arc::new(engine);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let router = iron_defer::http::router::build(Arc::clone(&engine));
    let token = CancellationToken::new();
    let server_token = token.clone();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_token.cancelled_owned())
            .await
            .expect("server");
    });

    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base_url}/tasks"))
        .json(&json!({
            "queue": queue,
            "kind": "region_e2e",
            "payload": {},
            "region": "eu-west"
        }))
        .send()
        .await
        .expect("create");
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let task_id = body["id"].as_str().unwrap();
    assert_eq!(body["region"], "eu-west");

    // Verify GET returns region
    let get_resp = client
        .get(format!("{base_url}/tasks/{task_id}"))
        .send()
        .await
        .expect("get");
    let get_body: serde_json::Value = get_resp.json().await.unwrap();
    assert_eq!(get_body["region"], "eu-west");

    token.cancel();
    let _ = server_handle.await;
}
