// Submission safety benchmarks for iron-defer (Story 9.3).
//
// Measures idempotency overhead (NFR-R7: < 5ms p99) and transactional
// enqueue overhead (NFR-R8: < 10ms p99) relative to baseline enqueue.
//
// Requires `DATABASE_URL` pointing to a running Postgres instance.
// Run: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench submission_safety

use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use iron_defer::{IronDefer, Task, TaskContext, TaskError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchTask;

impl Task for BenchTask {
    const KIND: &'static str = "bench_task";

    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

fn setup_pool_and_engine(
    rt: &tokio::runtime::Runtime,
) -> (sqlx::PgPool, IronDefer) {
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "DATABASE_URL is required for submission safety benchmarks.\n\
             Run: docker compose -f docker/docker-compose.dev.yml up -d\n\
             Then: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres \
             cargo bench --bench submission_safety"
        )
    });

    rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url)
            .await
            .expect("connect to Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run migrations");

        let engine = IronDefer::builder()
            .pool(pool.clone())
            .register::<BenchTask>()
            .skip_migrations(true)
            .build()
            .await
            .expect("build engine");

        (pool, engine)
    })
}

// ---------------------------------------------------------------------------
// NFR-R7: Idempotency overhead < 5ms at p99
// ---------------------------------------------------------------------------

fn idempotency_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let (pool, engine) = setup_pool_and_engine(&rt);

    let mut group = c.benchmark_group("idempotency_overhead");

    group.bench_function("baseline_enqueue", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let queue = format!("bench-base-{}", uuid::Uuid::new_v4());
                    let start = Instant::now();
                    engine.enqueue(&queue, BenchTask).await.expect("enqueue");
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.bench_function("idempotent_enqueue", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let key = uuid::Uuid::new_v4().to_string();
                    let queue = format!("bench-idemp-{}", uuid::Uuid::new_v4());
                    let start = Instant::now();
                    engine
                        .enqueue_idempotent(&queue, BenchTask, &key)
                        .await
                        .expect("enqueue_idempotent");
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.finish();

    // P99 latency report: collect individual timings
    let n = 500usize;
    let mut baseline_latencies = Vec::with_capacity(n);
    let mut idempotent_latencies = Vec::with_capacity(n);

    rt.block_on(async {
        for _ in 0..n {
            let queue = format!("bench-p99-base-{}", uuid::Uuid::new_v4());
            let start = Instant::now();
            engine.enqueue(&queue, BenchTask).await.expect("enqueue");
            baseline_latencies.push(start.elapsed());
        }

        for _ in 0..n {
            let key = uuid::Uuid::new_v4().to_string();
            let queue = format!("bench-p99-idemp-{}", uuid::Uuid::new_v4());
            let start = Instant::now();
            engine
                .enqueue_idempotent(&queue, BenchTask, &key)
                .await
                .expect("enqueue_idempotent");
            idempotent_latencies.push(start.elapsed());
        }
    });

    baseline_latencies.sort();
    idempotent_latencies.sort();

    let base_p99 = baseline_latencies[n * 99 / 100];
    let idemp_p99 = idempotent_latencies[n * 99 / 100];
    let overhead = idemp_p99.saturating_sub(base_p99);

    println!("\n=== Idempotency Overhead Report (NFR-R7) ===");
    println!("Sample size: {n}");
    println!("Baseline p99:    {base_p99:?}");
    println!("Idempotent p99:  {idemp_p99:?}");
    println!("Overhead p99:    {overhead:?}");
    println!("Target: < 5ms");
    println!(
        "Result: {}",
        if overhead < Duration::from_millis(5) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!("=============================================\n");

    // Duplicate detection benchmark
    let mut dedup_latencies = Vec::with_capacity(n);
    let dedup_queue = format!("bench-dedup-{}", uuid::Uuid::new_v4());
    let dedup_key = uuid::Uuid::new_v4().to_string();

    rt.block_on(async {
        // Seed one task
        engine
            .enqueue_idempotent(&dedup_queue, BenchTask, &dedup_key)
            .await
            .expect("seed");

        // Measure duplicate detection
        for _ in 0..n {
            let start = Instant::now();
            engine
                .enqueue_idempotent(&dedup_queue, BenchTask, &dedup_key)
                .await
                .expect("dedup");
            dedup_latencies.push(start.elapsed());
        }
    });

    dedup_latencies.sort();
    let dedup_p99 = dedup_latencies[n * 99 / 100];

    println!("=== Duplicate Detection Report ===");
    println!("Sample size: {n}");
    println!("Dedup p99: {dedup_p99:?}");
    println!("==================================\n");

    drop(engine);
    rt.block_on(pool.close());
}

// ---------------------------------------------------------------------------
// NFR-R8: Transactional enqueue overhead < 10ms at p99
// ---------------------------------------------------------------------------

fn transactional_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let (pool, engine) = setup_pool_and_engine(&rt);

    let mut group = c.benchmark_group("transactional_overhead");

    group.bench_function("baseline_enqueue", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let queue = format!("bench-tx-base-{}", uuid::Uuid::new_v4());
                    let start = Instant::now();
                    engine.enqueue(&queue, BenchTask).await.expect("enqueue");
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.bench_function("tx_enqueue", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let queue = format!("bench-tx-{}", uuid::Uuid::new_v4());
                    let start = Instant::now();
                    let mut tx = pool.begin().await.expect("begin");
                    engine
                        .enqueue_in_tx(&mut tx, &queue, BenchTask, None)
                        .await
                        .expect("enqueue_in_tx");
                    tx.commit().await.expect("commit");
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.finish();

    // P99 latency report
    let n = 500usize;
    let mut baseline_latencies = Vec::with_capacity(n);
    let mut tx_latencies = Vec::with_capacity(n);

    rt.block_on(async {
        for _ in 0..n {
            let queue = format!("bench-p99-tx-base-{}", uuid::Uuid::new_v4());
            let start = Instant::now();
            engine.enqueue(&queue, BenchTask).await.expect("enqueue");
            baseline_latencies.push(start.elapsed());
        }

        for _ in 0..n {
            let queue = format!("bench-p99-tx-{}", uuid::Uuid::new_v4());
            let start = Instant::now();
            let mut tx = pool.begin().await.expect("begin");
            engine
            .enqueue_in_tx(&mut tx, &queue, BenchTask, None)

                .await
                .expect("enqueue_in_tx");
            tx.commit().await.expect("commit");
            tx_latencies.push(start.elapsed());
        }
    });

    baseline_latencies.sort();
    tx_latencies.sort();

    let base_p99 = baseline_latencies[n * 99 / 100];
    let tx_p99 = tx_latencies[n * 99 / 100];
    let overhead = tx_p99.saturating_sub(base_p99);

    println!("\n=== Transactional Overhead Report (NFR-R8) ===");
    println!("Sample size: {n}");
    println!("Baseline p99:     {base_p99:?}");
    println!("Tx enqueue p99:   {tx_p99:?}");
    println!("Overhead p99:     {overhead:?}");
    println!("Target: < 10ms");
    println!(
        "Result: {}",
        if overhead < Duration::from_millis(10) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    println!("===============================================\n");

    drop(engine);
    rt.block_on(pool.close());
}

criterion_group!(benches, idempotency_overhead, transactional_overhead);
criterion_main!(benches);
