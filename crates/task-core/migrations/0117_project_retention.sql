-- 第 4 フェーズ P4-D（ADR 2026-11-05-phase4-p4d-retention）: wake・判断・compare の記録の保持と GC。
--
-- events は追記専用の正本のまま触らない。消すのは派生の行のうち、条件を満たすものだけ:
--   * `project_wakes`: 消費済み（consumed_by あり）の `cron` の wake。未消費の wake は消さない
--     （0115 が表を作り直すので wake には trigger を置かず、GC の DELETE 文の条件で守る）。
--   * `project_controller_decisions`: 効果 `none` の判断。pending・applied・suppressed・failed、案件の最後の
--     判断（`project_controller_state.last_decision_id`）、再開（`pc/resume/%`）・許可待ちの判断は消さない。
--   * `script_compare_log`: TS が適用した行（applied_by = ts。見回りの違反の根拠）は消さない。
-- 消す行は先に archive（JSONL）に書いて fsync し、その sha256 と件数を `project_retention_runs` に残す。
-- GC は同じ transaction の中で `project_retention_permit` に 1 行入れてから消し、commit の前に消す。
-- 許可の行が無い DELETE（手で打った DELETE・不具合）は trigger が拒む。

CREATE TABLE IF NOT EXISTS project_retention_permit (
    table_name TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS project_retention_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    table_name TEXT NOT NULL,
    archived INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    first_key TEXT,
    last_key TEXT,
    cutoff TEXT NOT NULL,
    archive_path TEXT,
    archive_sha256 TEXT
);
CREATE TRIGGER IF NOT EXISTS project_retention_runs_no_update
BEFORE UPDATE ON project_retention_runs
BEGIN
    SELECT RAISE(ABORT, 'project_retention_runs is append-only');
END;
CREATE TRIGGER IF NOT EXISTS project_retention_runs_no_delete
BEFORE DELETE ON project_retention_runs
BEGIN
    SELECT RAISE(ABORT, 'project_retention_runs is append-only');
END;

-- 「案件の最後の判断か」を GC の SELECT・DELETE・trigger が 1 行ごとに引く。
CREATE INDEX IF NOT EXISTS project_controller_state_last_decision
    ON project_controller_state (last_decision_id) WHERE last_decision_id IS NOT NULL;

DROP TRIGGER IF EXISTS project_controller_decisions_no_delete;
CREATE TRIGGER project_controller_decisions_no_delete
BEFORE DELETE ON project_controller_decisions
WHEN NOT EXISTS (SELECT 1 FROM project_retention_permit WHERE table_name = 'project_controller_decisions')
  OR OLD.effect_state <> 'none'
  OR OLD.validation LIKE 'pc/resume/%'
  OR OLD.validation = 'pc/authority-required'
  OR EXISTS (SELECT 1 FROM project_controller_state s WHERE s.last_decision_id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'project_controller_decisions is append-only (retention GC may delete effect none rows only)');
END;

DROP TRIGGER IF EXISTS script_compare_log_no_delete;
CREATE TRIGGER script_compare_log_no_delete
BEFORE DELETE ON script_compare_log
WHEN NOT EXISTS (SELECT 1 FROM project_retention_permit WHERE table_name = 'script_compare_log')
  OR COALESCE(json_extract(OLD.json, '$.applied_by'), '') = 'ts'
BEGIN
    SELECT RAISE(ABORT, 'script_compare_log is append-only (retention GC may delete rows not applied by ts only)');
END;
