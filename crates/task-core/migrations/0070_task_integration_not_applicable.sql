-- Allow the sweep to record repositories it deliberately does not inspect.
CREATE TABLE task_integrations_new (
    task_id TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('unintegrated','integrated','discarded','held','not_applicable')),
    reason TEXT,
    actor TEXT NOT NULL,
    checked_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id)
);
INSERT INTO task_integrations_new
    (task_id,repo_id,branch,head_sha,state,reason,actor,checked_at,updated_at)
SELECT task_id,repo_id,branch,head_sha,state,reason,actor,checked_at,updated_at
FROM task_integrations;
DROP TABLE task_integrations;
ALTER TABLE task_integrations_new RENAME TO task_integrations;
CREATE INDEX idx_task_integrations_disposition
    ON task_integrations (state, checked_at DESC);
