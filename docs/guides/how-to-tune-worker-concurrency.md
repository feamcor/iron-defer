# How to Tune Worker Concurrency

## Goal

Increase throughput without causing pool saturation or latency regressions.

## When to Use

Use this when queue backlog grows or worker utilization remains near saturation.

## Prerequisites

- baseline access to `/queues` and `/metrics`
- ability to change `worker.concurrency` and `database.max_connections`

## Steps

1. Capture baseline metrics:

   - queue depth (`GET /queues`)
   - `iron_defer_worker_pool_utilization`
   - `iron_defer_task_duration_seconds`
   - `iron_defer_claim_backoff_total`
   - DB pool active/idle counters

2. Increase `worker.concurrency` in small increments (for example `4 -> 8 -> 12`).

3. Re-measure after each increment and compare throughput and latency.

4. Adjust related controls if needed:

   - `worker.poll_interval`
   - `worker.max_claim_backoff`
   - `worker.shutdown_timeout`

## Verification

- pending backlog decreases under representative load
- failure rate and latency remain within target bounds
- pool saturation events do not increase materially

## Troubleshooting

- If backoff or pool pressure rises, lower concurrency or raise `database.max_connections`.
- If latency worsens, profile handlers and downstream dependencies before further scaling.
