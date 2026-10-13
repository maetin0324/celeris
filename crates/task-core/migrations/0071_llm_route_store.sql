ALTER TABLE tasks ADD COLUMN harness_id TEXT;
ALTER TABLE runs ADD COLUMN selected_harness_id TEXT;
ALTER TABLE runs ADD COLUMN selected_source_id TEXT;
ALTER TABLE runs ADD COLUMN selected_model_id TEXT;
ALTER TABLE runs ADD COLUMN selected_account_id TEXT;
ALTER TABLE runs ADD COLUMN selection_metadata_json TEXT;

CREATE TABLE account_limits_new (
    source_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    max_runs INTEGER NOT NULL CHECK (max_runs BETWEEN 1 AND 32),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (source_id, account_id)
);
INSERT INTO account_limits_new (source_id, account_id, max_runs, updated_at)
SELECT CASE adapter WHEN 'claude-code' THEN 'claude_oauth' WHEN 'codex' THEN 'codex_oauth' ELSE adapter END,
       account_id, max_runs, updated_at
FROM account_limits;
DROP TABLE account_limits;
ALTER TABLE account_limits_new RENAME TO account_limits;

UPDATE tasks SET harness_id = json_extract(json, '$.worker_hint.adapter')
WHERE json_type(json, '$.worker_hint.adapter') = 'text';
