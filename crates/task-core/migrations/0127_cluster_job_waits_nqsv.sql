-- Migration 127 (schema_version=127): ADR 2026-11-24-cluster-job-wait-nqsv D6。
--
-- `cluster_job_waits.scheduler` に NEC NQSV（`nqsv`）を足す。SQLite は CHECK を変えられないので表を作り直す。
-- 行はそのまま写し（消さない）、index を作り直す。列と意味は migration 0034 と同じ。
CREATE TABLE cluster_job_waits_new (
    wait_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    work_unit_id TEXT,
    run_id TEXT NOT NULL,
    cluster TEXT NOT NULL,
    scheduler TEXT NOT NULL CHECK (scheduler IN ('pbs', 'slurm', 'nqsv')),
    jobs_json TEXT NOT NULL,
    poll_secs INTEGER NOT NULL,
    timeout_secs INTEGER NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    checkpoint_json TEXT,
    created_at TEXT NOT NULL,
    deadline TEXT NOT NULL,
    last_polled_at TEXT,
    finished_at TEXT,
    state TEXT NOT NULL CHECK (state IN ('waiting', 'satisfied', 'timed_out', 'cancelled')),
    last_status_json TEXT NOT NULL DEFAULT '[]'
);
INSERT INTO cluster_job_waits_new (wait_id, task_id, work_unit_id, run_id, cluster, scheduler, jobs_json,
    poll_secs, timeout_secs, summary, checkpoint_json, created_at, deadline, last_polled_at, finished_at,
    state, last_status_json)
    SELECT wait_id, task_id, work_unit_id, run_id, cluster, scheduler, jobs_json, poll_secs, timeout_secs,
        summary, checkpoint_json, created_at, deadline, last_polled_at, finished_at, state, last_status_json
    FROM cluster_job_waits;
DROP TABLE cluster_job_waits;
ALTER TABLE cluster_job_waits_new RENAME TO cluster_job_waits;
CREATE INDEX IF NOT EXISTS idx_cluster_job_waits_task ON cluster_job_waits (task_id, created_at);
CREATE INDEX IF NOT EXISTS idx_cluster_job_waits_state ON cluster_job_waits (state);
