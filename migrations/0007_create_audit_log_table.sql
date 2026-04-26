-- Append-only audit log for task state transitions (Story 10.2 / FR55, FR56).
-- NFR-C1: immutability enforced via database trigger.
-- NFR-C2: audit INSERT is in the same transaction as the state change.

CREATE TABLE task_audit_log (
    id         BIGSERIAL    PRIMARY KEY,
    task_id    UUID         NOT NULL REFERENCES tasks(id),
    from_status TEXT,
    to_status  TEXT         NOT NULL,
    timestamp  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    worker_id  UUID,
    trace_id   VARCHAR,
    metadata   JSONB
);

CREATE INDEX idx_audit_log_task_time ON task_audit_log (task_id, timestamp);

CREATE OR REPLACE FUNCTION audit_log_immutable()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'audit log is append-only: % operations are forbidden', TG_OP;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_audit_log_immutable
    BEFORE UPDATE OR DELETE ON task_audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_immutable();

-- Automatically audit status transitions on the tasks table.
-- This ensures that even direct SQL updates to 'status' are captured.
CREATE OR REPLACE FUNCTION audit_task_status_change()
RETURNS TRIGGER AS $$
BEGIN
    IF (OLD.status IS DISTINCT FROM NEW.status) THEN
        INSERT INTO task_audit_log (task_id, from_status, to_status, worker_id, trace_id, metadata)
        VALUES (
            NEW.id,
            OLD.status,
            NEW.status,
            OLD.claimed_by, -- Use the worker who was holding the task during the transition
            NEW.trace_id,
            jsonb_build_object('trigger', 'database_after_update')
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_audit_task_status_change
    AFTER UPDATE ON tasks
    FOR EACH ROW EXECUTE FUNCTION audit_task_status_change();
