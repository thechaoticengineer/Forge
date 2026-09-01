ALTER TABLE task_commits RENAME TO task_commits_before_approval;

CREATE TABLE task_commits (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    worktree_id TEXT NOT NULL REFERENCES task_worktrees(id) ON DELETE RESTRICT,
    implementation_attempt_id TEXT NOT NULL REFERENCES implementation_attempts(id) ON DELETE RESTRICT,
    verification_attempt_id TEXT NOT NULL REFERENCES verification_attempts(id) ON DELETE RESTRICT,
    review_attempt_id TEXT NOT NULL REFERENCES review_attempts(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (
        status IN ('proposed', 'reserved', 'created', 'rejected', 'stale', 'failed')
    ),
    message TEXT NOT NULL,
    tree_hash TEXT,
    changed_files_json TEXT,
    patch TEXT,
    commit_hash TEXT,
    error_message TEXT,
    decision_reason TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

INSERT INTO task_commits(
    id, run_id, task_id, worktree_id, implementation_attempt_id,
    verification_attempt_id, review_attempt_id, status, message, tree_hash,
    changed_files_json, patch, commit_hash, error_message, decision_reason,
    created_at, completed_at
)
SELECT
    id, run_id, task_id, worktree_id, implementation_attempt_id,
    verification_attempt_id, review_attempt_id, status, message, NULL,
    NULL, NULL, commit_hash, error_message, NULL, created_at, completed_at
FROM task_commits_before_approval;

DROP TABLE task_commits_before_approval;

CREATE INDEX task_commits_run_created_idx ON task_commits(run_id, created_at DESC);
