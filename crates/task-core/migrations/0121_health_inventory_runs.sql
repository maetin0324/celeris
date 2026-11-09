-- ADR 2026-11-08-health-enforcement-loop D6.5: append-only repository inventory.
CREATE TABLE health_inventory_runs (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL,
    scheduled_for TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('evaluated', 'unchanged', 'not_applicable')),
    evidence_json TEXT,
    task_ids_json TEXT NOT NULL,
    existing_skipped INTEGER NOT NULL CHECK (existing_skipped >= 0),
    limit_remaining INTEGER NOT NULL CHECK (limit_remaining >= 0),
    created_at TEXT NOT NULL,
    CHECK ((outcome = 'evaluated' AND evidence_json IS NOT NULL)
        OR (outcome != 'evaluated' AND evidence_json IS NULL))
);
CREATE INDEX health_inventory_job_repo ON health_inventory_runs(job_id, repo_id);
