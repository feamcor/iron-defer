# Tutorial: Retries and Failures

## Objective

Observe retry scheduling and terminal failure behavior.

## Prerequisites

- local instance running from [First Task Local](first-task-local.md)
- a task handler that can intentionally fail

## Steps

1. Submit a task expected to fail:

   ```sh
   curl -sS -X POST http://localhost:8080/tasks \
     -H 'content-type: application/json' \
     -d '{"queue":"default","kind":"failing_task","payload":{},"maxAttempts":3}'
   ```

2. Poll task state:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>"
   ```

3. Inspect retry metadata fields:

   - `attempts`
   - `maxAttempts`
   - `lastError`
   - `scheduledAt`

4. Inspect logs and metrics:

   - lifecycle events: `task_failed_retry`, `task_failed_terminal`
   - metrics: `iron_defer_task_attempts_total`, `iron_defer_task_failures_total`

## Verification

- state sequence includes `pending -> running -> pending` during retries
- final state becomes `failed` after max attempts
- retry and failure counters increment

## Next Steps

- Continue with [Suspended Workflow](suspended-workflow.md)
- See [How to Debug Stuck Tasks](../guides/how-to-debug-stuck-tasks.md)
