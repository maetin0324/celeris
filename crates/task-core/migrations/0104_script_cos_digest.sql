-- ADR 2026-11-02-ts-control-layer-boundary 付記 S4-4・S4-5: 領域 `cos_digest`（CoS の受信箱 digest pass）。
-- `script_domain_modes` — `cos_digest` の行を mode `rust`・version 0 で足す。
-- `script_compare_log` — thread・run に属する compare の記録（追記専用）。`events.task_id` は NOT NULL で、
--   digest は task でなく thread・run に属するので events に書かない。本文は残さない（S4-5）。

INSERT OR IGNORE INTO script_domain_modes (domain, mode, generation_id, version, updated_at, updated_by)
VALUES ('cos_digest', 'rust', NULL, 0, '1970-01-01T00:00:00Z', 'system:migration');

CREATE TABLE IF NOT EXISTS script_compare_log (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    domain TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS script_compare_log_run ON script_compare_log (thread_id, run_id, id);
CREATE INDEX IF NOT EXISTS script_compare_log_domain ON script_compare_log (domain, id);
CREATE TRIGGER IF NOT EXISTS script_compare_log_no_update
BEFORE UPDATE ON script_compare_log
BEGIN
    SELECT RAISE(ABORT, 'script_compare_log is append-only');
END;
CREATE TRIGGER IF NOT EXISTS script_compare_log_no_delete
BEFORE DELETE ON script_compare_log
BEGIN
    SELECT RAISE(ABORT, 'script_compare_log is append-only');
END;
