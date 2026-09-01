CREATE TABLE review_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    worktree_id TEXT NOT NULL REFERENCES task_worktrees(id) ON DELETE RESTRICT,
    implementation_attempt_id TEXT NOT NULL
        REFERENCES implementation_attempts(id) ON DELETE RESTRICT,
    implementer TEXT NOT NULL CHECK (implementer IN ('codex', 'claude')),
    reviewer TEXT NOT NULL CHECK (reviewer IN ('codex', 'claude')),
    policy TEXT NOT NULL CHECK (
        policy IN ('cross_provider_required', 'cross_provider_or_fresh_session')
    ),
    independence TEXT NOT NULL CHECK (
        independence IN ('cross_provider', 'fresh_session_fallback')
    ),
    status TEXT NOT NULL CHECK (
        status IN ('running', 'approved', 'changes_requested', 'blocked', 'failed')
    ),
    prompt TEXT NOT NULL CHECK (length(trim(prompt)) > 0),
    result_json TEXT,
    final_output TEXT,
    diagnostic_output TEXT,
    exit_code INTEGER,
    error_message TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

CREATE INDEX review_attempts_run_started_idx
    ON review_attempts(run_id, started_at DESC);

CREATE UNIQUE INDEX review_attempts_running_run_idx
    ON review_attempts(run_id)
    WHERE status = 'running';
