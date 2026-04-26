// Audit log overhead benchmark for iron-defer (Story 10.3, Task 7).
//
// Measures throughput (tasks/sec) with audit_log = false vs audit_log = true.
// Informational quality gate.
//
// Requires `DATABASE_URL` pointing to a running Postgres instance.
// Run: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench audit_overhead

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use iron_defer::{
    CancellationToken, DatabaseConfig, IronDefer, Task, TaskContext, TaskError, WorkerConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditBenchTask;

impl Task for AuditBenchTask {
    const KIND: &'static str = "audit_bench_task";
    async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
        Ok(())
    }
}

const BATCH_SIZE: usize = 100; // Smaller batch for more samples
const BATCH_TIMEOUT: Duration = Duration::from_secs(10);
const OVERHEAD_THRESHOLD_PCT: f64 = 20.0;

fn audit_overhead_benchmark(c: &mut Criterion) {
    let database_url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL is required for the audit overhead benchmark.\n\
         Example: DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres"
    );

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
        // Ensure migrations are up to date including the removal of the redundant trigger
        IronDefer::migrator()
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    });

    let mut group = c.benchmark_group("audit_overhead");
    group.sample_size(10);

    group.bench_function("throughput_audit_off", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_batch(&pool, false).await;
                }
                total
            })
        });
    });

    group.bench_function("throughput_audit_on", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_batch(&pool, true).await;
                }
                total
            })
        });
    });

    group.finish();

    // Summary report
    let off_dur = rt.block_on(run_batch(&pool, false));
    let on_dur = rt.block_on(run_batch(&pool, true));
    let off_rate = BATCH_SIZE as f64 / off_dur.as_secs_f64();
    let on_rate = BATCH_SIZE as f64 / on_dur.as_secs_f64();
    let overhead_pct = ((off_rate - on_rate) / off_rate) * 100.0;
    
    println!("\n=== Audit Log Overhead Report ===");
    println!("Batch size: {BATCH_SIZE}");
    println!("audit_log=false: {off_rate:.1} tasks/sec");
    println!("audit_log=true:  {on_rate:.1} tasks/sec");
    println!("Overhead: {overhead_pct:.1}% (Target: <= {OVERHEAD_THRESHOLD_PCT}%)");
    println!(
        "Result: {}",
        if overhead_pct <= OVERHEAD_THRESHOLD_PCT { "PASS" } else { "FAIL (Informational)" }
    );
    println!("==================================\n");
}

async fn run_batch(pool: &sqlx::PgPool, audit_log: bool) -> Duration {
    let queue = format!("bench-audit-{}", uuid::Uuid::new_v4());
    let engine = IronDefer::builder()
        .pool(pool.clone())
        .register::<AuditBenchTask>()
        .worker_config(WorkerConfig {
            concurrency: 8,
            poll_interval: Duration::from_millis(5),
            shutdown_timeout: Duration::from_secs(2),
            ..WorkerConfig::default()
        })
        .database_config(DatabaseConfig {
            audit_log,
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
            .enqueue(&queue, AuditBenchTask)
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

criterion_group!(benches, audit_overhead_benchmark);
criterion_main!(benches);
