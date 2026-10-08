-- Telemetry only. Existing resolved_config_json preserves harness/model/session mode.
ALTER TABLE chat_runs ADD COLUMN usage_json TEXT CHECK (usage_json IS NULL OR json_valid(usage_json));
ALTER TABLE chat_runs ADD COLUMN skill_reads INTEGER CHECK (skill_reads IS NULL OR skill_reads >= 0);
ALTER TABLE chat_runs ADD COLUMN first_output_at TEXT;
CREATE INDEX chat_runs_thread_history ON chat_runs(thread_id);
