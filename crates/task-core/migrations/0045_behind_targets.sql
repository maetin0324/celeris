-- ADR-0130 D4. Last behind measurement of a task branch against its current target, per repo.
-- `behind_commits` NULL = Git/ref unreadable (never stored as 0).
-- `behind_since` = first UTC observation of a positive behind in the same `target_ref` series.
CREATE TABLE task_behind_targets (
    task_id TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    target_ref TEXT NOT NULL,
    target_sha TEXT,
    head_sha TEXT,
    behind_commits INTEGER,
    behind_since TEXT,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id)
);
