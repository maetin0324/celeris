-- Preserve the legacy change/PR attempt history while assigning task_integrations
-- to the durable task-by-repository disposition required by ADR 2026-10-11.
ALTER TABLE task_integrations RENAME TO task_change_integrations;

CREATE TABLE task_integrations (
    task_id TEXT NOT NULL,
    repo_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('unintegrated','integrated','discarded','held')),
    reason TEXT,
    actor TEXT NOT NULL,
    checked_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id)
);
CREATE INDEX idx_task_integrations_disposition
    ON task_integrations (state, checked_at DESC);
