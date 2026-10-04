-- ADR-0130 D1-D2. Explicit task hints live in their own table so the task JSON
-- (and every `Task` initializer) stays unchanged. A WU hint is the /3 plan
-- unit's `expected_write_paths` (read from the plan), else the task's hint.
CREATE TABLE task_write_hints (
    task_id TEXT PRIMARY KEY,
    paths_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE run_write_sets (
    run_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    work_unit_id TEXT,
    repo_id TEXT NOT NULL,
    base_sha TEXT,
    head_sha TEXT,
    paths_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('complete', 'incomplete', 'unavailable')),
    reason TEXT,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (run_id, repo_id)
);
CREATE INDEX idx_run_write_sets_task ON run_write_sets(task_id);
CREATE INDEX idx_run_write_sets_work_unit ON run_write_sets(work_unit_id);

CREATE TABLE work_unit_write_sets (
    work_unit_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    base_sha TEXT,
    head_sha TEXT,
    paths_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('complete', 'incomplete', 'unavailable')),
    reason TEXT,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, repo_id)
);
CREATE INDEX idx_work_unit_write_sets_task ON work_unit_write_sets(task_id);
