-- Extend the status CHECK constraint to include 'suspended' (G7 HITL).
-- Without this, any UPDATE setting status='suspended' will be rejected.
ALTER TABLE tasks DROP CONSTRAINT tasks_status_check;
ALTER TABLE tasks ADD CONSTRAINT tasks_status_check
    CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled', 'suspended'));

ALTER TABLE tasks ADD COLUMN suspended_at TIMESTAMPTZ DEFAULT now();
-- For existing rows that might be transitioned to suspended later,
-- we'll rely on the app layer to set suspended_at correctly,
-- but a default provides a safety net for the watchdog.
ALTER TABLE tasks ADD COLUMN signal_payload JSONB;
