# Tutorial: First Task Local

## Objective

Run iron-defer locally, submit one task, and verify end-to-end execution.

## Prerequisites

- Docker available locally
- Rust toolchain installed
- repository checked out

## Steps

1. Start Postgres:

   ```sh
   docker compose -f docker/docker-compose.dev.yml up -d
   ```

2. Start the server:

   ```sh
   DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
   cargo run -p iron-defer -- serve --port 8080
   ```

3. Check health endpoints:

   ```sh
   curl -sS http://localhost:8080/health
   curl -sS http://localhost:8080/health/ready
   ```

4. Submit a task:

   ```sh
   curl -sS -X POST http://localhost:8080/tasks \
     -H 'content-type: application/json' \
     -d '{"kind":"demo","payload":{"hello":"world"}}'
   ```

5. Query the task:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>"
   ```

6. Inspect queue and metrics:

   ```sh
   curl -sS http://localhost:8080/queues
   curl -sS http://localhost:8080/metrics
   ```

7. Stop services when done:

   ```sh
   docker compose -f docker/docker-compose.dev.yml down
   ```

## Verification

- `/health` returns `{}`
- `/health/ready` returns `{"status":"ready","db":"ok"}`
- task transitions from `pending` to `running` and eventually terminal state

## Next Steps

- Continue with [Operate Standalone](operate-standalone.md)
- Continue with [Retries and Failures](retries-and-failures.md)
