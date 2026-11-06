-- ADR 2026-11-06-cos-run-launch-accounting-and-drain D1 / ADR 2026-11-06-chat-run-launch-boundary-store.
-- Existing rows are NOT backfilled: launched_at / prelaunch_reason stay NULL and launch_tracked stays 0
-- (boundary unknown, never treated as "not launched"). usage_json, credentials and events are untouched.
ALTER TABLE chat_runs ADD COLUMN launched_at TEXT;
ALTER TABLE chat_runs ADD COLUMN prelaunch_reason TEXT;
ALTER TABLE chat_runs ADD COLUMN launch_tracked INTEGER NOT NULL DEFAULT 0 CHECK (launch_tracked IN (0, 1));
ALTER TABLE chat_runs ADD COLUMN attempts_recorded INTEGER NOT NULL DEFAULT 0 CHECK (attempts_recorded >= 0);
ALTER TABLE chat_runs ADD COLUMN attempts_usage_missing INTEGER NOT NULL DEFAULT 0 CHECK (attempts_usage_missing >= 0);
