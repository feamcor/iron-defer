# REST API Reference

iron-defer exposes a REST API for task submission, lifecycle management, queue visibility, and health/metrics probing.

- Base path: `/`
- OpenAPI spec: `GET /openapi.json` (OpenAPI 3.1)
- JSON naming: camelCase
- Default request body limit: 1 MiB

## Endpoints

### POST /tasks

Create a task.

Request body:

```json
{
  "queue": "default",
  "kind": "send_email",
  "payload": {"to": "user@example.com"},
  "scheduledAt": "2030-01-01T00:00:00Z",
  "priority": 5,
  "maxAttempts": 10,
  "idempotencyKey": "orders:1234:email",
  "region": "us-east-1"
}
```

- Required fields: `kind`, `payload`
- Optional fields: `queue` (defaults to `default`), `scheduledAt`, `priority`, `maxAttempts`, `idempotencyKey`, `region`
- If `idempotencyKey` matches an existing non-terminal task in the same queue, the server returns the existing task (`200 OK`) instead of creating a duplicate.

Headers:

- Optional `traceparent`: if valid, the trace id is persisted on the task for correlation.

Responses:

- `201 Created` for a newly created task
- `200 OK` for idempotent replay returning an existing task
- `422 Unprocessable Entity` for validation errors

### GET /tasks/{id}

Fetch one task by UUID.

- `200 OK` with task payload
- `404 Not Found` if unknown

### DELETE /tasks/{id}

Cancel a task.

- Cancels only pending tasks
- Running/suspended/terminal tasks are not cancellable

Responses:

- `200 OK` when cancelled
- `404 Not Found` if unknown
- `409 Conflict` if the task is not cancellable in its current state

### POST /tasks/{id}/signal

Resume a suspended task and optionally attach a signal payload consumed by the next execution attempt.

Request body:

```json
{
  "payload": {"decision": "approved", "by": "reviewer-42"}
}
```

Responses:

- `200 OK` when resumed
- `404 Not Found` if unknown
- `409 Conflict` if the task is not suspended

### GET /tasks/{id}/audit

Read audit log entries for a task.

Query parameters:

| Param | Description |
|---|---|
| `limit` | Entries per page (default 100, clamped to 1..1000) |
| `offset` | Offset (default 0) |

Response shape:

```json
{
  "entries": [
    {
      "id": 42,
      "taskId": "550e8400-e29b-41d4-a716-446655440000",
      "fromStatus": "pending",
      "toStatus": "running",
      "timestamp": "2026-04-24T12:34:56Z",
      "workerId": "4f79e53b-a573-4f86-a1b9-2dad77d8f9cf",
      "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
      "metadata": null
    }
  ],
  "total": 1,
  "limit": 100,
  "offset": 0
}
```

### GET /tasks

List tasks with filters and pagination.

Query parameters:

| Param | Description |
|---|---|
| `queue` | Queue filter |
| `status` | Status filter (`pending`, `running`, `completed`, `failed`, `cancelled`, `suspended`; case-insensitive) |
| `limit` | Page size (default 50, clamped to 1..100) |
| `offset` | Pagination offset (capped at 10,000) |

Important behavior:

- Unfiltered queries are rejected. You must provide `queue` or `status`.
- `offset` values above 10,000 are capped server-side.

Response shape:

```json
{
  "tasks": [],
  "total": 0,
  "limit": 50,
  "offset": 0
}
```

### GET /queues

Queue statistics.

Query parameters:

| Param | Description |
|---|---|
| `byRegion` | When `true`, returns one row per `(queue, region)`; default `false` |

Response shape:

```json
[
  {
    "queue": "default",
    "region": "us-east-1",
    "pending": 12,
    "running": 3,
    "suspended": 1,
    "activeWorkers": 2
  }
]
```

### GET /health

Liveness probe. Always returns `200 OK` with `{}` while the process is alive.

### GET /health/ready

Readiness probe. Executes a database check.

- `200 OK` when healthy
- `503 Service Unavailable` when DB is unavailable or probe times out

Example healthy response:

```json
{"status":"ready","db":"ok"}
```

### GET /metrics

Prometheus text exposition output.

- `200 OK` with `text/plain; version=0.0.4; charset=utf-8`
- `404 Not Found` if metrics registry is not configured

### GET /openapi.json

OpenAPI 3.1 document.

## Error Format

All error responses use:

```json
{"error":{"code":"ERROR_CODE","message":"human-readable description"}}
```

Common codes:

| Code | Typical Status | Meaning |
|---|---|---|
| `INVALID_PAYLOAD` | 422 | Invalid task body or field constraints |
| `INVALID_QUERY_PARAMETER` | 422 | Invalid query/filter value |
| `TASK_NOT_FOUND` | 404 | Task id not found |
| `TASK_ALREADY_CLAIMED` | 409 | Task is running and cannot be cancelled |
| `TASK_IN_TERMINAL_STATE` | 409 | Task is completed/failed/cancelled |
| `TASK_SUSPENDED` | 409 | Task is suspended and operation requires another state |
| `TASK_NOT_IN_EXPECTED_STATE` | 409 | Transition precondition failed |
| `INTERNAL_ERROR` | 500 | Unexpected server failure |
