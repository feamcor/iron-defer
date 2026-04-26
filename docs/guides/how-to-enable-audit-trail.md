# How to Enable Audit Trail

## Goal

Enable durable task transition history for operational forensics and compliance evidence.

## When to Use

Use this when you need traceable task lifecycle records beyond transient logs.

## Prerequisites

- write access to configuration
- running service with reachable Postgres

## Steps

1. Configure audit mode:

   ```toml
   [database]
   audit_log = true
   unlogged_tables = false
   ```

   or:

   ```sh
   IRON_DEFER__DATABASE__AUDIT_LOG=true
   IRON_DEFER__DATABASE__UNLOGGED_TABLES=false
   ```

2. Validate resolved config:

   ```sh
   iron-defer config validate
   ```

3. Run a known task lifecycle and query audit entries:

   ```sh
   curl -sS "http://localhost:8080/tasks/<task-id>/audit?limit=100&offset=0"
   ```

## Verification

- audit endpoint returns transition records for processed tasks
- transition sequence includes expected states (`pending -> running -> <terminal>`)

## Troubleshooting

- If validation fails, ensure `audit_log=true` is not combined with `unlogged_tables=true`.
- If no entries appear, confirm the task was processed after audit mode was enabled.
