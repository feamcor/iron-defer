-- Harden task_audit_log schema (Story 10.2 review finding).
-- NFR-C1: Apply reasonable length constraints to prevent resource exhaustion.

ALTER TABLE task_audit_log ALTER COLUMN trace_id TYPE VARCHAR(255);
