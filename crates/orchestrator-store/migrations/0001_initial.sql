CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL,
    repository_path TEXT NOT NULL UNIQUE,
    git_common_dir TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_opened_at INTEGER NOT NULL
) STRICT;

CREATE TABLE runs (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
    goal TEXT NOT NULL CHECK (length(trim(goal)) > 0),
    base_revision TEXT NOT NULL,
    branch TEXT,
    worktree_dirty INTEGER NOT NULL CHECK (worktree_dirty IN (0, 1)),
    status TEXT NOT NULL CHECK (
        status IN (
            'draft',
            'planning',
            'waiting_for_user',
            'running',
            'blocked',
            'failed',
            'completed',
            'rejected',
            'cancelled'
        )
    ),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX runs_project_created_idx ON runs(project_id, created_at DESC);
CREATE INDEX runs_status_updated_idx ON runs(status, updated_at DESC);

CREATE TABLE run_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX run_events_run_sequence_idx ON run_events(run_id, sequence);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    relative_path TEXT NOT NULL UNIQUE,
    media_type TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX artifacts_run_created_idx ON artifacts(run_id, created_at);
