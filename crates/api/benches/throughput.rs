// Throughput benchmark for iron-defer task processing.
//
// Requires `DATABASE_URL` pointing to a running Postgres instance.
// Run: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench throughput
// Compile check only (no DB): cargo bench --bench throughput --no-run

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use iron_defer::{CancellationToken, IronDefer, Task, TaskContext, TaskError, WorkerConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoopTask;

impl Task for NoopTask {
    const KIND: &'static str = "noop_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

const BATCH_SIZE: usize = 1000;

#[allow(clippy::too_many_lines)]
fn throughput_benchmark(c: &mut Criterion) {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "DATABASE_URL is required for the throughput benchmark.\n\
             Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
             Then: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench throughput"
        )
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let pool = rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url)
            .await
            .expect("connect to Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    });

    c.bench_function("task_throughput", |b| {
        b.iter_custom(|iters| {
            let mut total_elapsed = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let queue = format!("bench-{}", uuid::Uuid::new_v4());

                    let config = WorkerConfig {
                        concurrency: 16,
                        poll_interval: Duration::from_millis(10),
                        sweeper_interval: Duration::from_mins(1),
                        shutdown_timeout: Duration::from_secs(5),
                        ..WorkerConfig::default()
                    };

                    let engine = IronDefer::builder()
                        .pool(pool.clone())
                        .register::<NoopTask>()
                        .worker_config(config)
                        .queue(&queue)
                        .build()
                        .await
                        .expect("build engine");

                    for _ in 0..BATCH_SIZE {
                        engine.enqueue(&queue, NoopTask).await.expect("enqueue");
                    }

                    let token = CancellationToken::new();
                    let engine = Arc::new(engine);
                    let engine_bg = engine.clone();
                    let token_bg = token.clone();

                    let start = Instant::now();

                    let handle = tokio::spawn(async move {
                        let _ = engine_bg.start(token_bg).await;
                    });

                    // Poll until all tasks complete.
                    loop {
                        let completed: i64 = sqlx::query_scalar(
                            "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'",
                        )
                        .bind(&queue)
                        .fetch_one(&pool)
                        .await
                        .expect("count");

                        if completed >= i64::try_from(BATCH_SIZE).expect("fits") {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }

                    let elapsed = start.elapsed();

                    token.cancel();
                    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

                    elapsed
                });

                total_elapsed += elapsed;
            }

            total_elapsed
        });
    });

    let final_throughput = rt.block_on(async {
        let queue = format!("bench-final-{}", uuid::Uuid::new_v4());
        let config = WorkerConfig {
            concurrency: 16,
            poll_interval: Duration::from_millis(10),
            sweeper_interval: Duration::from_mins(1),
            shutdown_timeout: Duration::from_secs(5),
            ..WorkerConfig::default()
        };

        let engine = IronDefer::builder()
            .pool(pool.clone())
            .register::<NoopTask>()
            .worker_config(config)
            .queue(&queue)
            .build()
            .await
            .expect("build");

        for _ in 0..BATCH_SIZE {
            engine.enqueue(&queue, NoopTask).await.expect("enqueue");
        }

        let token = CancellationToken::new();
        let engine = Arc::new(engine);
        let engine_bg = engine.clone();
        let token_bg = token.clone();

        let start = Instant::now();
        let handle = tokio::spawn(async move {
            let _ = engine_bg.start(token_bg).await;
        });

        loop {
            let completed: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tasks WHERE queue = $1 AND status = 'completed'",
            )
            .bind(&queue)
            .fetch_one(&pool)
            .await
            .expect("count");

            if completed >= i64::try_from(BATCH_SIZE).expect("fits") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let elapsed = start.elapsed();
        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

        #[allow(clippy::cast_precision_loss)]
        let throughput = BATCH_SIZE as f64 / elapsed.as_secs_f64();
        throughput
    });

    println!("\n=== Throughput Report ===");
    println!("Batch size: {BATCH_SIZE}");
    println!("Throughput: {final_throughput:.0} tasks/sec");
    println!("Target (NFR-P2): >= 10,000 tasks/sec");
    println!("========================\n");
}

#[allow(clippy::too_many_lines)]
fn claim_latency_benchmark(c: &mut Criterion) {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "DATABASE_URL is required for the claim latency benchmark.\n\
             Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
             Then: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench throughput"
        )
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let pool = rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url)
            .await
            .expect("connect to Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    });

    c.bench_function("claim_latency", |b| {
        b.iter_custom(|iters| {
            let mut total_elapsed = Duration::ZERO;

            for _ in 0..iters {
                let elapsed = rt.block_on(async {
                    let queue = format!("bench-claim-{}", uuid::Uuid::new_v4());

                    for _ in 0..100 {
                        let task = NoopTask;
                        let payload = serde_json::to_value(&task).expect("serialize");
                        sqlx::query(
                            "INSERT INTO tasks (id, queue, kind, status, payload, priority, \
                             max_attempts, scheduled_at, created_at, updated_at) \
                             VALUES ($1, $2, $3, 'pending', $4, 0, 3, now(), now(), now())",
                        )
                        .bind(uuid::Uuid::new_v4())
                        .bind(&queue)
                        .bind(NoopTask::KIND)
                        .bind(payload)
                        .execute(&pool)
                        .await
                        .expect("seed task");
                    }

                    let worker_id = uuid::Uuid::new_v4();
                    let start = Instant::now();

                    for _ in 0..100 {
                        let _row: Option<(uuid::Uuid,)> = sqlx::query_as(
                            "UPDATE tasks SET status = 'running', \
                             claimed_by = $1, \
                             attempts = attempts + 1, \
                             lease_expires_at = now() + interval '30 seconds', \
                             updated_at = now() \
                             WHERE id = ( \
                               SELECT id FROM tasks \
                               WHERE queue = $2 AND status = 'pending' \
                                 AND scheduled_at <= now() \
                               ORDER BY priority DESC, scheduled_at ASC \
                               LIMIT 1 \
                               FOR UPDATE SKIP LOCKED \
                             ) \
                             RETURNING id",
                        )
                        .bind(worker_id)
                        .bind(&queue)
                        .fetch_optional(&pool)
                        .await
                        .expect("claim");
                    }

                    start.elapsed()
                });

                total_elapsed += elapsed;
            }

            total_elapsed
        });
    });

    let latencies = rt.block_on(async {
        let queue = format!("bench-p99-{}", uuid::Uuid::new_v4());
        let n = 1000usize;

        for _ in 0..n {
            let task = NoopTask;
            let payload = serde_json::to_value(&task).expect("serialize");
            sqlx::query(
                "INSERT INTO tasks (id, queue, kind, status, payload, priority, \
                 max_attempts, scheduled_at, created_at, updated_at) \
                 VALUES ($1, $2, $3, 'pending', $4, 0, 3, now(), now(), now())",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(&queue)
            .bind(NoopTask::KIND)
            .bind(payload)
            .execute(&pool)
            .await
            .expect("seed task");
        }

        let worker_id = uuid::Uuid::new_v4();
        let mut durations = Vec::with_capacity(n);

        for _ in 0..n {
            let start = Instant::now();
            let _row: Option<(uuid::Uuid,)> = sqlx::query_as(
                "UPDATE tasks SET status = 'running', \
                 claimed_by = $1, \
                 attempts = attempts + 1, \
                 lease_expires_at = now() + interval '30 seconds', \
                 updated_at = now() \
                 WHERE id = ( \
                   SELECT id FROM tasks \
                   WHERE queue = $2 AND status = 'pending' \
                     AND scheduled_at <= now() \
                   ORDER BY priority DESC, scheduled_at ASC \
                   LIMIT 1 \
                   FOR UPDATE SKIP LOCKED \
                 ) \
                 RETURNING id",
            )
            .bind(worker_id)
            .bind(&queue)
            .fetch_optional(&pool)
            .await
            .expect("claim");
            durations.push(start.elapsed());
        }

        durations.sort();
        durations
    });

    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];

    println!("\n=== Claim Latency Report ===");
    println!("Sample size: {}", latencies.len());
    println!("P50: {p50:?}");
    println!("P95: {p95:?}");
    println!("P99: {p99:?}");
    println!("============================\n");
}

const DISPATCH_COUNT: usize = 10_000;

fn dispatch_strategy_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let mut group = c.benchmark_group("dispatch_strategy");

    group.bench_function("tokio_spawn", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += rt.block_on(async {
                    let start = Instant::now();
                    let mut handles = Vec::with_capacity(DISPATCH_COUNT);
                    for _ in 0..DISPATCH_COUNT {
                        handles.push(tokio::spawn(async {}));
                    }
                    for h in handles {
                        let _ = h.await;
                    }
                    start.elapsed()
                });
            }
            total
        });
    });

    group.bench_function("catch_unwind", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                for _ in 0..DISPATCH_COUNT {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {}));
                }
                total += start.elapsed();
            }
            total
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    throughput_benchmark,
    claim_latency_benchmark,
    dispatch_strategy_benchmark
);
criterion_main!(benches);
