-- 第 4 フェーズ P4-B（ADR 2026-11-05-phase4-p4b-evidence-revocation）: wake の種類 `evidence_changed` を足す。
--
-- achieved の案件の根拠の root が reopen（done から非終端へ）・取り込みの reopen（IntegrationReopened）に
-- なったとき、controller を起こして Rust が達成を数え直す。SQLite は CHECK を変えられないので表を作り直す。
-- 行（未消費の wake・消費の記録）はそのまま写す（消さない）。
CREATE TABLE project_wakes_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    wake_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'root_done', 'root_failed', 'review_failed', 'children_settled', 'decision_answered',
        'external_released', 'stagnation', 'cron', 'human_changed', 'evidence_changed')),
    source_event_id INTEGER,
    source_ref TEXT,
    created_at TEXT NOT NULL,
    consumed_by TEXT,
    UNIQUE (project_id, wake_key)
);
INSERT INTO project_wakes_new (id, project_id, wake_key, kind, source_event_id, source_ref,
    created_at, consumed_by)
    SELECT id, project_id, wake_key, kind, source_event_id, source_ref, created_at, consumed_by
    FROM project_wakes;
DROP TABLE project_wakes;
ALTER TABLE project_wakes_new RENAME TO project_wakes;
CREATE INDEX IF NOT EXISTS project_wakes_pending
    ON project_wakes (project_id, id) WHERE consumed_by IS NULL;
