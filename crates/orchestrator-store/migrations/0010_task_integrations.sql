CREATE TABLE task_integrations (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    task_commit_id TEXT NOT NULL REFERENCES task_commits(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    target_branch TEXT NOT NULL,
    expected_head TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('reserved', 'completed', 'failed')),
    result_head TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

CREATE INDEX task_integrations_run_created_idx
    ON task_integrations(run_id, created_at DESC);
CREATE INDEX task_integrations_commit_created_idx
    ON task_integrations(task_commit_id, created_at DESC);
