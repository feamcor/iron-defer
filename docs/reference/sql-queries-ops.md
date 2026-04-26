# SQL Queries for Operations

## Scope

Operational SQL snippets for incident triage and reporting against task tables.

## Canonical Tables/Entries

### Queue status snapshot

```sql
SELECT queue, status, count(*) AS n
FROM tasks
GROUP BY queue, status
ORDER BY queue, status;
```

### Old running tasks (possible zombies)

```sql
SELECT id, queue, kind, claimed_by, claimed_until, updated_at
FROM tasks
WHERE status = 'running'
  AND claimed_until < now()
ORDER BY claimed_until ASC;
```

### Retry-heavy tasks

```sql
SELECT id, queue, kind, attempts, max_attempts, last_error, updated_at
FROM tasks
WHERE attempts > 0
ORDER BY attempts DESC, updated_at DESC
LIMIT 100;
```

### Suspended backlog

```sql
SELECT id, queue, kind, suspended_at, updated_at
FROM tasks
WHERE status = 'suspended'
ORDER BY suspended_at ASC NULLS LAST;
```

### Audit entries for one task

```sql
SELECT id, task_id, from_status, to_status, timestamp, worker_id, trace_id, metadata
FROM task_audit_log
WHERE task_id = $1
ORDER BY id ASC;
```

### Time-window task transitions

```sql
SELECT to_status, count(*) AS n
FROM task_audit_log
WHERE timestamp >= $1
  AND timestamp <  $2
GROUP BY to_status
ORDER BY to_status;
```

## Related Docs

- [How to Debug Stuck Tasks](../guides/how-to-debug-stuck-tasks.md)
- [How to Enable Audit Trail](../guides/how-to-enable-audit-trail.md)
