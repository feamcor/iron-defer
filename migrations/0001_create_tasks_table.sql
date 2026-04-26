-- Story 1A.2: tasks table + claiming/zombie indexes (architecture §D1.1, lines 269-298).
-- gen_random_uuid() lives in pgcrypto on Postgres < 13 and is built into core
-- on >= 13. The testcontainers postgres image (16) ships pgcrypto enabled by
-- default but we add the extension explicitly for portability across older
-- managed Postgres deployments.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE tasks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    queue         TEXT NOT NULL DEFAULT 'default',
    kind          TEXT NOT NULL,
    payload       JSONB NOT NULL DEFAULT '{}',
    status        TEXT NOT NULL DEFAULT 'pending',
    priority      SMALLINT NOT NULL DEFAULT 0,
    attempts      INTEGER NOT NULL DEFAULT 0,
    max_attempts  INTEGER NOT NULL DEFAULT 3,
    last_error    TEXT,
    scheduled_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_by    UUID,
    claimed_until TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Status enum guard: rejects typos and case-mismatches at the storage
    -- boundary so a corrupt row cannot become invisible to the claiming
    -- index and quarantined behind a generic mapping error.
    CONSTRAINT tasks_status_check
        CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
    -- Non-empty kind guard: empty kinds are otherwise accepted on INSERT
    -- and rejected only at the read-side TryFrom mapping, which leaves an
    -- orphaned row that fails every subsequent read.
    CONSTRAINT tasks_kind_nonempty_check
        CHECK (length(kind) > 0)
);

-- Claiming index: pending tasks eligible for pickup, ordered by priority + scheduled_at.
-- Used by Story 1B.1 SKIP LOCKED claim query.
CREATE INDEX idx_tasks_claiming
    ON tasks (queue, status, priority DESC, scheduled_at ASC)
    WHERE status = 'pending';

-- Sweeper index: running tasks with expired leases.
-- Used by Story 2.1 zombie recovery sweeper.
CREATE INDEX idx_tasks_zombie
    ON tasks (status, claimed_until)
    WHERE status = 'running';
