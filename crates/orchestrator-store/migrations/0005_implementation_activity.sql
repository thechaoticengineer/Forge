CREATE TABLE implementation_activity (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id TEXT NOT NULL REFERENCES implementation_attempts(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    agent TEXT NOT NULL CHECK (agent IN ('codex', 'claude')),
    kind TEXT NOT NULL CHECK (kind IN ('output', 'diagnostic')),
    message TEXT NOT NULL CHECK (
        length(message) > 0 AND length(message) <= 8192
    ),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX implementation_activity_run_sequence_idx
    ON implementation_activity(run_id, sequence DESC);

CREATE INDEX implementation_activity_attempt_sequence_idx
    ON implementation_activity(attempt_id, sequence);
