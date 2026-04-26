-- Story 9.1: Idempotency key support
-- Adds optional idempotency_key and idempotency_expires_at columns to enable
-- exactly-once submission semantics per queue.

ALTER TABLE tasks ADD COLUMN idempotency_key VARCHAR;
ALTER TABLE tasks ADD COLUMN idempotency_expires_at TIMESTAMPTZ;

-- Partial unique index: scoped per-queue, only for active (non-terminal) tasks
-- with a non-NULL key. The WHERE predicate must match the ON CONFLICT clause
-- in save_idempotent() exactly (Postgres requires textual match).
CREATE UNIQUE INDEX idx_tasks_idempotency
    ON tasks (queue, idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status NOT IN ('completed', 'failed', 'cancelled');
