-- Story 10.1 Review: Add index for trace_id to support efficient correlation
-- and prevent performance bottlenecks as the tasks table grows.
CREATE INDEX IF NOT EXISTS idx_tasks_trace_id ON tasks (trace_id) WHERE trace_id IS NOT NULL;
