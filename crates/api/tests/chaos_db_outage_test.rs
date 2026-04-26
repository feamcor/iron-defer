//! Chaos test: Database outage survival (AC 2, TEA P1-CHAOS-002).
//!
//! Postgres becomes unavailable during worker polling. Workers retry on
//! reconnection. No tasks are lost.
//!
//! Moved from `db_outage_integration_test.rs` — this is the definitive
//! DB outage chaos test.

mod chaos_common;

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use iron_defer_application::DatabaseConfig;
use iron_defer_infrastructure::create_pool;
use serde::{Deserialize, Serialize};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const TOTAL: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CountingTask {
    n: usize,
}

impl Task for CountingTask {
    const KIND: &'static str = "counting_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn postgres_outage_survives_reconnection() {
    if chaos_common::should_skip() {
        eprintln!("[skip] IRON_DEFER_SKIP_DOCKER_CHAOS set");
        return;
    }

    let (pool, container, _url, pre_outage_port) = chaos_common::boot_isolated_chaos_db().await;

    COUNTER.store(0, Ordering::SeqCst);

    let queue = chaos_common::unique_queue();

    let config = WorkerConfig {
        concurrency: 2,
        poll_interval: Duration::from_millis(100),
        sweeper_interval: Duration::from_millis(500),
        shutdown_timeout: Duration::from_secs(5),
        lease_duration: Duration::from_secs(5),
        ..WorkerConfig::default()
    };

    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<CountingTask>()
        .worker_config(config)
        .queue(&queue)
        .build()
        .await
        .expect("build engine");

    for i in 0..TOTAL {
        engine
            .enqueue(&queue, CountingTask { n: i })
            .await
            .expect("enqueue");
    }

    let token = CancellationToken::new();
    let engine = Arc::new(engine);
    let engine_bg = engine.clone();
    let token_bg = token.clone();
    let engine_task = tokio::spawn(async move {
        if let Err(e) = engine_bg.start(token_bg).await {
            eprintln!("[engine] exited with error: {e}");
        }
    });

    // Let some tasks be processed before the outage.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- OUTAGE WINDOW ---
    container.stop().await.expect("stop container");
    tokio::time::sleep(Duration::from_secs(3)).await;
    container.start().await.expect("restart container");

    let restart_port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("get host port after restart");
    assert_eq!(
        pre_outage_port, restart_port,
        "Docker reassigned the host port after container restart \
         (pre={pre_outage_port}, post={restart_port}) — reconnection cannot be validated"
    );
    let post_restart_url =
        format!("postgres://postgres:postgres@127.0.0.1:{restart_port}/postgres");
    // --- END OUTAGE ---

    let completion = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if COUNTER.load(Ordering::SeqCst) >= TOTAL {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;

    token.cancel();
    let engine_join = tokio::time::timeout(Duration::from_secs(10), engine_task).await;

    // On timeout, query task table via a fresh pool to surface per-task
    // diagnostic so CI logs show exactly what's stuck.
    if completion.is_err() {
        let diag_pool_result = create_pool(&DatabaseConfig {
            url: post_restart_url.clone(),
            max_connections: 2,
            ..Default::default()
        })
        .await;
        let diag_pool_err = diag_pool_result.as_ref().err().map(|e| format!("{e}"));
        let diag_pool = diag_pool_result.ok();

        let mut diagnostic = format!(
            "tasks did not complete within 30s after reconnection (counter = {})\n",
            COUNTER.load(Ordering::SeqCst)
        );
        if let Some(dp) = diag_pool {
            let rows: Vec<(String, String, i32, Option<String>)> = sqlx::query_as(
                "SELECT id::text, status, attempts, claimed_by::text \
                 FROM tasks WHERE queue = $1 ORDER BY created_at",
            )
            .bind(&queue)
            .fetch_all(&dp)
            .await
            .unwrap_or_default();

            let (stuck, terminal): (Vec<_>, Vec<_>) = rows.iter().partition(|(_, status, _, _)| {
                status != "completed" && status != "failed" && status != "cancelled"
            });

            let _ = write!(diagnostic, "Non-terminal tasks ({}):\n", stuck.len());
            for (id, status, attempts, claimed_by) in &stuck {
                let _ = write!(
                    diagnostic,
                    "  id={id} status={status} attempts={attempts} claimed_by={}\n",
                    claimed_by.as_deref().unwrap_or("NULL")
                );
            }
            let _ = write!(
                diagnostic,
                "Terminal tasks: {} completed/failed\n",
                terminal.len()
            );
        } else {
            let _ = write!(
                diagnostic,
                "(could not create diagnostic pool: {})",
                diag_pool_err.as_deref().unwrap_or("unknown error")
            );
        }
        panic!("{diagnostic}");
    }

    match engine_join {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) => panic!("engine task panicked during or after outage: {join_err}"),
        Err(elapsed) => panic!("engine task did not exit within 10s after cancellation: {elapsed}"),
    }

    // Verify via fresh pool (the engine's pool may still hold broken connections).
    drop(pool);
    let verify_pool = create_pool(&DatabaseConfig {
        url: post_restart_url.clone(),
        max_connections: 4,
        ..Default::default()
    })
    .await
    .expect("create verification pool");

    let completed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'")
            .bind(&queue)
            .fetch_one(&verify_pool)
            .await
            .expect("count completed");
    assert_eq!(
        completed,
        i64::try_from(TOTAL).expect("fits"),
        "expected all {TOTAL} tasks completed, got {completed}"
    );

    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'running'")
            .bind(&queue)
            .fetch_one(&verify_pool)
            .await
            .expect("count running");
    assert_eq!(running, 0, "expected zero running tasks, got {running}");
}
