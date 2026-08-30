CREATE TABLE task_worktrees (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (
        status IN ('reserved', 'ready', 'missing', 'failed', 'retired')
    ),
    branch TEXT NOT NULL CHECK (length(trim(branch)) > 0),
    path TEXT NOT NULL CHECK (length(trim(path)) > 0),
    base_revision TEXT NOT NULL CHECK (length(base_revision) = 40),
    repository_dirty INTEGER NOT NULL CHECK (repository_dirty IN (0, 1)),
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX task_worktrees_run_created_idx
    ON task_worktrees(run_id, created_at);

-- A task holds at most one live worktree, branch, and directory. Failed,
-- missing, and retired records stay as history without blocking a retry.
CREATE UNIQUE INDEX task_worktrees_live_task_idx
    ON task_worktrees(run_id, task_id)
    WHERE status IN ('reserved', 'ready');

CREATE UNIQUE INDEX task_worktrees_live_branch_idx
    ON task_worktrees(branch)
    WHERE status IN ('reserved', 'ready');

CREATE UNIQUE INDEX task_worktrees_live_path_idx
    ON task_worktrees(path)
    WHERE status IN ('reserved', 'ready');
