CREATE TABLE verification_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    worktree_id TEXT NOT NULL REFERENCES task_worktrees(id) ON DELETE RESTRICT,
    implementation_attempt_id TEXT NOT NULL REFERENCES implementation_attempts(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'infrastructure_error')),
    commands_json TEXT NOT NULL,
    error_message TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER NOT NULL
) STRICT;

CREATE INDEX verification_attempts_run_started_idx
    ON verification_attempts(run_id, started_at DESC);

CREATE TABLE task_commits (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    worktree_id TEXT NOT NULL REFERENCES task_worktrees(id) ON DELETE RESTRICT,
    implementation_attempt_id TEXT NOT NULL REFERENCES implementation_attempts(id) ON DELETE RESTRICT,
    verification_attempt_id TEXT NOT NULL REFERENCES verification_attempts(id) ON DELETE RESTRICT,
    review_attempt_id TEXT NOT NULL REFERENCES review_attempts(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('reserved', 'created', 'failed')),
    message TEXT NOT NULL,
    commit_hash TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

CREATE INDEX task_commits_run_created_idx ON task_commits(run_id, created_at DESC);
