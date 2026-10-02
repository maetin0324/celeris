-- ADR-0130 D1-D2. Hints remain part of the canonical task/spec JSON; these
-- generated columns expose them to store queries without duplicating writes.
ALTER TABLE tasks ADD COLUMN expected_write_paths_json TEXT
    GENERATED ALWAYS AS (json_extract(json, '$.expected_write_paths')) VIRTUAL;
ALTER TABLE work_units ADD COLUMN expected_write_paths_json TEXT
    GENERATED ALWAYS AS (json_extract(json, '$.expected_write_paths')) VIRTUAL;

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
