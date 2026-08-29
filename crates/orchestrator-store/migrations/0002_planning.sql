ALTER TABLE runs ADD COLUMN last_error TEXT;

CREATE TABLE plan_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    agent TEXT NOT NULL CHECK (agent IN ('codex', 'claude')),
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    prompt TEXT NOT NULL CHECK (length(trim(prompt)) > 0),
    final_output TEXT,
    diagnostic_output TEXT,
    exit_code INTEGER,
    error_message TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
) STRICT;

CREATE INDEX plan_attempts_run_started_idx
    ON plan_attempts(run_id, started_at DESC);

CREATE TABLE plans (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    based_on_plan_id TEXT REFERENCES plans(id) ON DELETE RESTRICT,
    source_attempt_id TEXT REFERENCES plan_attempts(id) ON DELETE RESTRICT,
    planner_agent TEXT NOT NULL CHECK (planner_agent IN ('codex', 'claude')),
    status TEXT NOT NULL CHECK (status IN ('proposed', 'approved', 'rejected', 'superseded')),
    summary TEXT NOT NULL CHECK (length(trim(summary)) > 0),
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    UNIQUE(run_id, revision)
) STRICT;

CREATE INDEX plans_run_revision_idx ON plans(run_id, revision DESC);

CREATE TABLE plan_tasks (
    id TEXT PRIMARY KEY NOT NULL,
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK (position > 0),
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    description TEXT NOT NULL CHECK (length(trim(description)) > 0),
    UNIQUE(plan_id, position),
    UNIQUE(plan_id, id)
) STRICT;

CREATE INDEX plan_tasks_plan_position_idx ON plan_tasks(plan_id, position);

CREATE TABLE plan_acceptance_criteria (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES plan_tasks(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK (position > 0),
    criterion TEXT NOT NULL CHECK (length(trim(criterion)) > 0),
    UNIQUE(task_id, position)
) STRICT;

CREATE INDEX plan_criteria_task_position_idx
    ON plan_acceptance_criteria(task_id, position);

CREATE TABLE plan_task_dependencies (
    plan_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    PRIMARY KEY(task_id, depends_on_task_id),
    CHECK (task_id <> depends_on_task_id),
    FOREIGN KEY(plan_id, task_id) REFERENCES plan_tasks(plan_id, id) ON DELETE RESTRICT,
    FOREIGN KEY(plan_id, depends_on_task_id) REFERENCES plan_tasks(plan_id, id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX plan_dependencies_plan_idx ON plan_task_dependencies(plan_id);
