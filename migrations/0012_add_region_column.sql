-- Migration 0012: Add region column for geographic worker pinning
-- Also adds region to audit log for consistency
ALTER TABLE tasks ADD COLUMN region VARCHAR;
ALTER TABLE task_audit_log ADD COLUMN region VARCHAR;

CREATE INDEX idx_tasks_region_claiming
    ON tasks (queue, region, status, priority DESC, scheduled_at ASC)
    WHERE status = 'pending';
