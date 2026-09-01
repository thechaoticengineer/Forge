CREATE TABLE implementation_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    worktree_id TEXT NOT NULL REFERENCES task_worktrees(id) ON DELETE RESTRICT,
    agent TEXT NOT NULL CHECK (agent IN ('codex', 'claude')),
    status TEXT NOT NULL CHECK (
        status IN ('running', 'completed', 'failed', 'cancelled')
    ),
    prompt TEXT NOT NULL CHECK (length(trim(prompt)) > 0),
    final_output TEXT,
    diagnostic_output TEXT,
    exit_code INTEGER,
    error_message TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

CREATE INDEX implementation_attempts_run_started_idx
    ON implementation_attempts(run_id, started_at DESC);

-- The first workflow supervises one write-capable agent per run at a time.
CREATE UNIQUE INDEX implementation_attempts_running_run_idx
    ON implementation_attempts(run_id)
    WHERE status = 'running';
