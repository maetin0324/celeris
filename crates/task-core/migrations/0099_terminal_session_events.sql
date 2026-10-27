-- Migration 98（schema_version=98）: ADR 2026-10-27-web-terminal D4。
--
-- `terminal_session_events` — web 端末の開始・終了の監査（追記専用）。1 端末に `opened` と `closed` の 2 行。
-- 端末の入出力・token・パスワードは持たない。UPDATE・DELETE は trigger で拒む。
CREATE TABLE IF NOT EXISTS terminal_session_events (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('opened', 'closed')),
    target TEXT NOT NULL,
    actor TEXT NOT NULL,
    client TEXT,
    reason TEXT,
    duration_secs INTEGER,
    exit_code INTEGER,
    UNIQUE (session_id, kind)
);
CREATE INDEX IF NOT EXISTS terminal_session_events_ts ON terminal_session_events (ts);
CREATE TRIGGER IF NOT EXISTS terminal_session_events_no_update
BEFORE UPDATE ON terminal_session_events
BEGIN
    SELECT RAISE(ABORT, 'terminal_session_events is append-only');
END;
CREATE TRIGGER IF NOT EXISTS terminal_session_events_no_delete
BEFORE DELETE ON terminal_session_events
BEGIN
    SELECT RAISE(ABORT, 'terminal_session_events is append-only');
END;
