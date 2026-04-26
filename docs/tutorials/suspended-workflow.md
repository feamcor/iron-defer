# Tutorial: Suspended Workflow

## Objective

Exercise suspend and resume flow with `POST /tasks/{id}/signal`.

## Prerequisites

- local instance running from [First Task Local](first-task-local.md)
- a task handler that calls `ctx.suspend(...)`

## Steps

1. Submit a task that can suspend:

   ```sh
   curl -sS -X POST http://localhost:8080/tasks \
     -H 'content-type: application/json' \
     -d '{"queue":"default","kind":"approval_task","payload":{"requestId":"req-1"}}'
   ```

2. Confirm suspended state:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>"
   ```

3. Send signal payload:

   ```sh
   curl -sS -X POST "http://localhost:8080/tasks/<task-id>/signal" \
     -H 'content-type: application/json' \
     -d '{"payload":{"decision":"approved","actor":"ops-user"}}'
   ```

4. Poll task state again:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>"
   ```

## Verification

- task enters `suspended` before signal
- post-signal state sequence is `suspended -> pending -> running -> <terminal>`

## Next Steps

- See [How to Enable Audit Trail](../guides/how-to-enable-audit-trail.md)
- See [How to Debug Stuck Tasks](../guides/how-to-debug-stuck-tasks.md)
