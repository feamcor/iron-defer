use std::sync::Arc;
use std::time::Duration;
use criterion::{Criterion, criterion_group, criterion_main, BenchmarkId};
use iron_defer::IronDefer;
use iron_defer_domain::{QueueName, WorkerId};
use tokio::runtime::Runtime;

#[path = "../tests/common/mod.rs"]
mod common;

async fn setup_engine(queue: &str) -> IronDefer {
    let pool = common::fresh_pool_on_shared_container()
        .await
        .expect("pool");
    
    IronDefer::builder()
        .pool(pool)
        .queue(queue)
        .skip_migrations(false)
        .build()
        .await
        .expect("engine")
}

fn bench_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("runtime");
    let queue_str = "bench-throughput";
    let qn = QueueName::try_from(queue_str).unwrap();
    let engine = rt.block_on(setup_engine(queue_str));
    let engine = Arc::new(engine);

    let mut group = c.benchmark_group("geographic_pinning_throughput");
    let batch_size = 100;

    group.bench_with_input(BenchmarkId::new("unpinned_baseline", batch_size), &batch_size, |b, &n| {
        b.to_async(&rt).iter(|| async {
            // Enqueue n unpinned tasks
            for i in 0..n {
                engine.enqueue_raw(queue_str, "bench", serde_json::json!({"i": i}), None, None, None, None, None)
                    .await
                    .expect("enqueue");
            }

            // Claim all n tasks using 4 logical workers (to match the 4-region test concurrency)
            let workers: Vec<_> = (0..4).map(|_| WorkerId::new()).collect();
            let mut claimed = 0;
            while claimed < n {
                for worker_id in &workers {
                    if engine.claim_next(&qn, *worker_id, Duration::from_secs(30), None)
                        .await
                        .expect("claim")
                        .is_some() 
                    {
                        claimed += 1;
                    }
                }
            }
        });
    });

    group.bench_with_input(BenchmarkId::new("region_pinned_multi", batch_size), &batch_size, |b, &n| {
        b.to_async(&rt).iter(|| async {
            let regions = ["us-east", "us-west", "eu-central", "ap-south"];
            
            // Enqueue n tasks distributed across 4 regions
            for i in 0..n {
                let region = regions[i as usize % 4];
                engine.enqueue_raw(queue_str, "bench", serde_json::json!({"i": i}), None, None, None, None, Some(region))
                    .await
                    .expect("enqueue");
            }

            // Claim all n tasks using 4 regional workers
            let worker_ids: Vec<_> = (0..4).map(|_| WorkerId::new()).collect();
            let mut claimed = 0;
            while claimed < n {
                for i in 0..4 {
                    let region = regions[i];
                    let worker_id = worker_ids[i];
                    if engine.claim_next(&qn, worker_id, Duration::from_secs(30), Some(region))
                        .await
                        .expect("claim")
                        .is_some() 
                    {
                        claimed += 1;
                    }
                }
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_throughput);
criterion_main!(benches);
