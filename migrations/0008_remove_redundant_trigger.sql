-- Remove redundant audit trigger (Finding 1, Option B).
-- We rely on explicit application-level logging to avoid duplicate rows.

DROP TRIGGER IF EXISTS trg_audit_task_status_change ON tasks;
DROP FUNCTION IF EXISTS audit_task_status_change();
