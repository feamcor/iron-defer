// UNLOGGED table throughput benchmark for iron-defer (Story 11.2, Task 6).
//
// Compares enqueue + claim + complete throughput on LOGGED vs UNLOGGED tasks table.
// NFR-SC6 target: UNLOGGED delivers >= 5x throughput improvement.
//
// This benchmark must run on dedicated hardware with production-configured
// Postgres (tuned shared_buffers, max_wal_size). Testcontainers default
// config does NOT exhibit representative WAL overhead. Results in CI are
// not meaningful for NFR-SC6.
//
// Requires `DATABASE_URL` (LOGGED) and `DATABASE_URL_UNLOGGED` (UNLOGGED) env vars.
// Run: DATABASE_URL=... DATABASE_URL_UNLOGGED=... cargo bench --bench unlogged_throughput

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use iron_defer::{
    CancellationToken, DatabaseConfig, IronDefer, Task, TaskContext, TaskError, WorkerConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnloggedBenchTask;

impl Task for UnloggedBenchTask {
    const KIND: &'static str = "unlogged_bench_task";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

const BATCH_SIZE: usize = 1000;
const BATCH_TIMEOUT: Duration = Duration::from_secs(30);
const THROUGHPUT_TARGET_MULTIPLIER: f64 = 5.0;

fn unlogged_throughput_benchmark(c: &mut Criterion) {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required for the LOGGED run.\n\
         Example: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres",
    );
    let database_url_unlogged = std::env::var("DATABASE_URL_UNLOGGED").expect(
        "DATABASE_URL_UNLOGGED is required for the UNLOGGED run.\n\
         Example: DATABASE_URL_UNLOGGED=postgres://postgres:postgres@localhost:5433/postgres",
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let pool_logged = rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url)
            .await
            .expect("connect to logged Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run logged migrations");
        pool
    });

    let pool_unlogged = rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url_unlogged)
            .await
            .expect("connect to unlogged Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run unlogged migrations");
        pool
    });

    let mut group = c.benchmark_group("unlogged_throughput");
    group.sample_size(10);

    group.bench_function("throughput_logged", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_batch(&pool_logged, false).await;
                }
                total
            })
        });
    });

    group.bench_function("throughput_unlogged", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_batch(&pool_unlogged, true).await;
                }
                total
            })
        });
    });

    group.finish();

    // Summary report
    let logged_dur = rt.block_on(run_batch(&pool_logged, false));
    let unlogged_dur = rt.block_on(run_batch(&pool_unlogged, true));
    let logged_rate = BATCH_SIZE as f64 / logged_dur.as_secs_f64();
    let unlogged_rate = BATCH_SIZE as f64 / unlogged_dur.as_secs_f64();
    let speedup = unlogged_rate / logged_rate;

    println!("\n=== UNLOGGED Throughput Report ===");
    println!("Batch size: {BATCH_SIZE}");
    println!("LOGGED:   {logged_rate:.1} tasks/sec");
    println!("UNLOGGED: {unlogged_rate:.1} tasks/sec");
    println!("Speedup: {speedup:.1}x (Target: >= {THROUGHPUT_TARGET_MULTIPLIER}x)");
    println!(
        "Result: {}",
        if speedup >= THROUGHPUT_TARGET_MULTIPLIER {
            "PASS"
        } else {
            "FAIL (Informational — requires production-configured Postgres on dedicated hardware)"
        }
    );
    println!("===================================\n");
}

async fn run_batch(pool: &sqlx::PgPool, unlogged: bool) -> Duration {
    let queue = format!("bench-unlogged-{}", uuid::Uuid::new_v4());
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<UnloggedBenchTask>()
        .worker_config(WorkerConfig {
            concurrency: 8,
            poll_interval: Duration::from_millis(5),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .database_config(DatabaseConfig {
            unlogged_tables: unlogged,
            ..DatabaseConfig::default()
        })
        .queue(&queue)
        .skip_migrations(true)
        .build()
        .await
        .expect("build engine");

    let engine = Arc::new(engine);

    let mut task_ids = Vec::with_capacity(BATCH_SIZE);
    for _ in 0..BATCH_SIZE {
        let record = engine
            .enqueue(&queue, UnloggedBenchTask)
            .await
            .expect("enqueue");
        task_ids.push(record.id());
    }

    let token = CancellationToken::new();
    let cancel = token.clone();
    let eng = engine.clone();
    let worker = tokio::spawn(async move {
        let _ = eng.start(cancel).await;
    });

    let start = Instant::now();

    for id in task_ids {
        let deadline = Instant::now() + BATCH_TIMEOUT;
        loop {
            let record = engine.find(id).await.expect("find").expect("task exists");
            if record.status() == iron_defer::TaskStatus::Completed {
                break;
            }
            if Instant::now() > deadline {
                panic!("batch timed out waiting for task {id} to complete");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    let elapsed = start.elapsed();
    token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), worker).await;
    elapsed
}

criterion_group!(benches, unlogged_throughput_benchmark);
criterion_main!(benches);
