-- Story 1B.1: cross-field invariant for claim columns (AC 6).
-- Ensures claimed_by and claimed_until are always both NULL or both non-NULL.
-- Resolves deferred-work item: "(claimed_by, claimed_until) cross-field invariant unguarded".
ALTER TABLE tasks
    ADD CONSTRAINT tasks_claim_fields_check
    CHECK ((claimed_by IS NULL) = (claimed_until IS NULL));
