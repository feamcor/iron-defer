// Checkpoint persistence latency benchmark for iron-defer (Story 11.3, Task 5).
//
// Measures raw SQL UPDATE latency for checkpoint writes with varying payload
// sizes: 1 KiB, 10 KiB, 100 KiB, 1 MiB.
//
// NFR-R9 target: < 50ms at p99 for payloads up to 1 MiB.
//
// Requires `DATABASE_URL` pointing to a running Postgres instance.
// Run on reference benchmark environment for NFR-R9 validation.
// Run: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench checkpoint_latency

use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use iron_defer::IronDefer;
use serde_json::json;

const SAMPLE_COUNT: usize = 500;
const P99_TARGET_MS: f64 = 50.0;

fn checkpoint_latency_benchmark(c: &mut Criterion) {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required for the checkpoint latency benchmark.\n\
         Example: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres",
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let pool = rt.block_on(async {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await
            .expect("connect to Postgres");
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    });

    let payload_sizes: Vec<(&str, usize)> = vec![
        ("1_KiB", 1024),
        ("10_KiB", 10 * 1024),
        ("100_KiB", 100 * 1024),
        ("1_MiB", 1024 * 1024),
    ];

    let mut group = c.benchmark_group("checkpoint_latency");
    group.sample_size(100);

    for (label, size) in &payload_sizes {
        let payload = build_checkpoint_payload(*size);

        group.bench_function(format!("checkpoint_{label}"), |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let task_id = insert_running_task(&pool).await;
                        let start = Instant::now();
                        sqlx::query(
                            "UPDATE tasks SET checkpoint = $1, updated_at = now() WHERE id = $2",
                        )
                        .bind(&payload)
                        .bind(task_id)
                        .execute(&pool)
                        .await
                        .expect("checkpoint write");
                        total += start.elapsed();
                        cleanup_task(&pool, task_id).await;
                    }
                    total
                })
            });
        });
    }

    group.finish();

    // Detailed p99 report per payload size
    println!("\n=== Checkpoint Persistence Latency Report ===");
    println!("NFR-R9 target: < {P99_TARGET_MS}ms at p99 for payloads up to 1 MiB\n");

    for (label, size) in &payload_sizes {
        let payload = build_checkpoint_payload(*size);
        let mut latencies = Vec::with_capacity(SAMPLE_COUNT);

        rt.block_on(async {
            for _ in 0..SAMPLE_COUNT {
                let task_id = insert_running_task(&pool).await;
                let start = Instant::now();
                sqlx::query(
                    "UPDATE tasks SET checkpoint = $1, updated_at = now() WHERE id = $2",
                )
                .bind(&payload)
                .bind(task_id)
                .execute(&pool)
                .await
                .expect("checkpoint write");
                latencies.push(start.elapsed());
                cleanup_task(&pool, task_id).await;
            }
        });

        latencies.sort();
        let p50 = latencies[SAMPLE_COUNT / 2];
        let p99_idx = (SAMPLE_COUNT as f64 * 0.99) as usize;
        let p99 = latencies[p99_idx.min(SAMPLE_COUNT - 1)];
        let p99_ms = p99.as_secs_f64() * 1000.0;
        let pass = p99_ms < P99_TARGET_MS;

        println!(
            "  {label:>7}: p50={:.2}ms  p99={:.2}ms  {}",
            p50.as_secs_f64() * 1000.0,
            p99_ms,
            if pass { "PASS" } else { "FAIL" },
        );
    }

    println!(
        "\nNote: Run on reference benchmark environment for NFR-R9 validation."
    );
    println!("==========================================\n");
}

fn build_checkpoint_payload(size: usize) -> serde_json::Value {
    let data = "x".repeat(size);
    json!({"step": 1, "data": data})
}

async fn insert_running_task(pool: &sqlx::PgPool) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, queue, kind, payload, status, priority, attempts, max_attempts, scheduled_at, created_at, updated_at) \
         VALUES ($1, 'bench-checkpoint', 'bench_checkpoint', '{}'::jsonb, 'running', 0, 1, 3, now(), now(), now())",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("insert running task");
    id
}

async fn cleanup_task(pool: &sqlx::PgPool, task_id: uuid::Uuid) {
    sqlx::query("DELETE FROM tasks WHERE id = $1")
        .bind(task_id)
        .execute(pool)
        .await
        .expect("cleanup task");
}

criterion_group!(benches, checkpoint_latency_benchmark);
criterion_main!(benches);
