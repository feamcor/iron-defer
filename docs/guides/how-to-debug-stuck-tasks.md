# How to Debug Stuck Tasks

## Goal

Find why tasks remain in `pending`, `running`, or `suspended` longer than expected.

## When to Use

Use this during incidents where queue depth grows, tasks stall, or workers appear idle.

## Prerequisites

- access to REST endpoints (`/tasks`, `/queues`, `/metrics`)
- access to service logs

## Steps

1. Narrow scope with filtered task lists:

   ```sh
   curl -sS "http://localhost:8080/tasks?queue=<queue>&status=pending&limit=100&offset=0"
   curl -sS "http://localhost:8080/tasks?queue=<queue>&status=running&limit=100&offset=0"
   curl -sS "http://localhost:8080/tasks?queue=<queue>&status=suspended&limit=100&offset=0"
   ```

2. Check queue pressure and worker saturation:

   ```sh
   curl -sS "http://localhost:8080/queues?byRegion=true"
   curl -sS http://localhost:8080/metrics
   ```

3. Inspect one task deeply:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>"
   curl -sS "http://localhost:8080/tasks/<task-id>/audit?limit=100&offset=0"
   ```

4. Correlate logs by task id:

   ```sh
   RUST_LOG=info iron-defer 2>&1 | jq 'select(.task_id == "<task-id>")'
   ```

## Verification

- identified root cause category (capacity, handler failure, suspend backlog, or DB pressure)
- confirmed a mitigation action based on evidence

## Troubleshooting

- High `pending` + low `running`: increase `worker.concurrency` or DB pool capacity.
- Frequent retries: inspect `lastError` and retry/backoff configuration.
- Suspended backlog: issue `POST /tasks/{id}/signal` for resumable tasks.
