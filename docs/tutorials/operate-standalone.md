# Tutorial: Operate Standalone

## Objective

Run `iron-defer` in standalone mode and operate it through CLI and REST.

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

3. Submit a task with CLI (new terminal):

   ```sh
   DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
   cargo run -p iron-defer -- submit --queue default --kind demo --payload '{"source":"cli"}'
   ```

4. List tasks with CLI:

   ```sh
   DATABASE_URL=postgres://iron_defer:iron_defer@localhost:5432/iron_defer \
   cargo run -p iron-defer -- tasks --queue default --status pending
   ```

5. Query with REST:

   ```sh
   curl -sS "http://localhost:8080/tasks?queue=default&status=pending&limit=50&offset=0"
   curl -sS http://localhost:8080/queues
   ```

6. Check operator endpoints:

   ```sh
   curl -sS http://localhost:8080/health
   curl -sS http://localhost:8080/health/ready
   curl -sS http://localhost:8080/metrics
   ```

7. Stop with `Ctrl+C` in the server terminal and tear down Postgres:

   ```sh
   docker compose -f docker/docker-compose.dev.yml down
   ```

## Verification

- CLI and REST both show the same task states
- `/health` and `/health/ready` succeed
- `/metrics` exports Prometheus output

## Next Steps

- Continue with [Retries and Failures](retries-and-failures.md)
- See [Standalone Binary Guide](../guides/standalone-binary.md)
