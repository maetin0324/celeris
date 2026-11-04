-- ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap（レビューできる計画の版）。
--
-- 旧 milestones（ADR-0079 で凍結）とは別の名詞で、`milestones` 表・`tasks.milestone_id`・`projects.auto_advance`
-- を読まない。brief（`project_briefs`、動的な要約の全置換）とも別の表で、brief の書き込みは版を変えない。
-- 版を進める（`approved` にする）のは人の決定だけ。版の行と Outcome の記録は消せない（追記専用）。
-- 既存の案件には行を作らない（行が無い案件は今までどおり request の block を読む）。
CREATE TABLE IF NOT EXISTS project_roadmap_versions (
    project_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 1),
    status TEXT NOT NULL CHECK (status IN ('proposed', 'approved', 'rejected', 'superseded')),
    -- 提案の土台にした approved の版（無ければ NULL）。決定のときに今の approved と違えば 409。
    base_version INTEGER,
    request_sha256 TEXT NOT NULL,
    change_reason TEXT NOT NULL,
    outcomes_json TEXT NOT NULL,
    proposed_by TEXT NOT NULL,
    proposed_at TEXT NOT NULL,
    decided_by TEXT,
    decided_at TEXT,
    decision_note TEXT,
    superseded_at TEXT,
    PRIMARY KEY (project_id, version)
);
-- 案件ごとに approved の版は高々 1 つ。
CREATE UNIQUE INDEX IF NOT EXISTS project_roadmap_one_approved
    ON project_roadmap_versions (project_id) WHERE status = 'approved';

CREATE TRIGGER IF NOT EXISTS project_roadmap_versions_no_delete
BEFORE DELETE ON project_roadmap_versions
BEGIN
    SELECT RAISE(ABORT, 'project_roadmap_versions is append-only');
END;

-- 中身の欄は変えられない。status は proposed → approved | rejected（決定の欄を同時に埋める）と
-- approved → superseded（superseded_at を埋める）だけ進む。
CREATE TRIGGER IF NOT EXISTS project_roadmap_versions_update_guard
BEFORE UPDATE ON project_roadmap_versions
WHEN NEW.project_id IS NOT OLD.project_id
  OR NEW.version IS NOT OLD.version
  OR NEW.base_version IS NOT OLD.base_version
  OR NEW.request_sha256 IS NOT OLD.request_sha256
  OR NEW.change_reason IS NOT OLD.change_reason
  OR NEW.outcomes_json IS NOT OLD.outcomes_json
  OR NEW.proposed_by IS NOT OLD.proposed_by
  OR NEW.proposed_at IS NOT OLD.proposed_at
  OR NOT (
       (OLD.status = 'proposed' AND NEW.status IN ('approved', 'rejected')
          AND OLD.decided_by IS NULL AND NEW.decided_by IS NOT NULL AND NEW.decided_at IS NOT NULL
          AND NEW.superseded_at IS NULL)
    OR (OLD.status = 'approved' AND NEW.status = 'superseded'
          AND NEW.decided_by IS OLD.decided_by AND NEW.decided_at IS OLD.decided_at
          AND NEW.decision_note IS OLD.decision_note AND NEW.superseded_at IS NOT NULL)
  )
BEGIN
    SELECT RAISE(ABORT, 'project_roadmap_versions: only proposed->approved|rejected and approved->superseded');
END;

-- Outcome の状態の変化と root との対応の記録（追記専用）。今の状態は Outcome ごとに最後の `status` の行。
CREATE TABLE IF NOT EXISTS project_roadmap_outcome_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    outcome_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('status', 'root_linked')),
    status TEXT CHECK (status IS NULL OR status IN ('open', 'in_progress', 'reached')),
    root_task_id TEXT,
    roadmap_version INTEGER NOT NULL,
    evidence_json TEXT NOT NULL DEFAULT '[]',
    reason TEXT NOT NULL DEFAULT '',
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK ((kind = 'status' AND status IS NOT NULL) OR (kind = 'root_linked' AND root_task_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS project_roadmap_outcome_events_project
    ON project_roadmap_outcome_events (project_id, outcome_id, id);

CREATE TRIGGER IF NOT EXISTS project_roadmap_outcome_events_no_update
BEFORE UPDATE ON project_roadmap_outcome_events
BEGIN
    SELECT RAISE(ABORT, 'project_roadmap_outcome_events is append-only');
END;

CREATE TRIGGER IF NOT EXISTS project_roadmap_outcome_events_no_delete
BEFORE DELETE ON project_roadmap_outcome_events
BEGIN
    SELECT RAISE(ABORT, 'project_roadmap_outcome_events is append-only');
END;
