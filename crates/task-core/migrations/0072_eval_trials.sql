-- ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-dispatch）:
-- 評価 run 1 試行 = 1 行。check_cmd は採点専用で worker に渡さない。
CREATE TABLE eval_trials (
    trial_id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL,
    manifest_sha TEXT NOT NULL,
    problem_id TEXT NOT NULL,
    trial_no INTEGER NOT NULL CHECK (trial_no >= 0),
    source_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    harness_version TEXT,
    repo TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    objective TEXT NOT NULL,
    check_cmd TEXT NOT NULL,
    check_timeout_secs INTEGER NOT NULL CHECK (check_timeout_secs > 0),
    status TEXT NOT NULL CHECK (status IN
        ('queued','running','scoring','passed','failed','error','cancelled')),
    run_id TEXT,
    decision_id TEXT,
    worktree_path TEXT,
    check_exit INTEGER,
    timed_out INTEGER NOT NULL DEFAULT 0 CHECK (timed_out IN (0, 1)),
    error TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    UNIQUE (manifest_sha, problem_id, trial_no, source_id, model_id, harness_id)
);
CREATE INDEX idx_eval_trials_status ON eval_trials(status, created_at);
CREATE INDEX idx_eval_trials_batch ON eval_trials(batch_id);
