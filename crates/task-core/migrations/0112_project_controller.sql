-- ADR 2026-11-04-project-controller D12・A1: 案件ごとの durable wake と判断の記録。
--
-- task の状態は持たない（I1）。`project_wakes` は events 等から作る派生で、別のイベントストアではない（I2）。
-- 状態の行が無い案件は mode `off` と同じ（I13）。
CREATE TABLE IF NOT EXISTS project_controller_state (
    project_id TEXT PRIMARY KEY,
    mode TEXT NOT NULL DEFAULT 'off' CHECK (mode IN ('off', 'dry_run', 'shadow', 'enabled')),
    phase TEXT NOT NULL DEFAULT 'active'
        CHECK (phase IN ('active', 'waiting', 'blocked', 'paused', 'achieved', 'cancelled')),
    revision INTEGER NOT NULL DEFAULT 0,
    event_cursor INTEGER NOT NULL DEFAULT 0,
    next_wake_at TEXT,
    coalesce_until TEXT,
    lease_owner TEXT,
    lease_expires_at TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    backoff_until TEXT,
    last_decision_id TEXT,
    human_epoch INTEGER NOT NULL DEFAULT 0,
    progress_epoch INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project_wakes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    wake_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'root_done', 'root_failed', 'review_failed', 'children_settled', 'decision_answered',
        'external_released', 'stagnation', 'cron', 'human_changed')),
    source_event_id INTEGER,
    source_ref TEXT,
    created_at TEXT NOT NULL,
    consumed_by TEXT,
    UNIQUE (project_id, wake_key)
);
CREATE INDEX IF NOT EXISTS project_wakes_pending
    ON project_wakes (project_id, id) WHERE consumed_by IS NULL;

CREATE TABLE IF NOT EXISTS project_controller_decisions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    based_on_revision INTEGER NOT NULL,
    human_epoch INTEGER NOT NULL,
    snapshot_sha256 TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    generation_id TEXT,
    mode TEXT NOT NULL CHECK (mode IN ('dry_run', 'shadow', 'enabled')),
    proposal_json TEXT NOT NULL,
    validation TEXT NOT NULL,
    effect_json TEXT NOT NULL,
    effect_state TEXT NOT NULL CHECK (effect_state IN ('none', 'pending', 'applied', 'suppressed', 'failed')),
    effect_ref TEXT,
    progress_epoch INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    applied_at TEXT
);
CREATE INDEX IF NOT EXISTS project_controller_decisions_project
    ON project_controller_decisions (project_id, created_at, id);
CREATE INDEX IF NOT EXISTS project_controller_decisions_pending
    ON project_controller_decisions (effect_state) WHERE effect_state = 'pending';

-- 判断の記録は effect_state・effect_ref・applied_at の前進だけを UPDATE できる。DELETE はできない。
CREATE TRIGGER IF NOT EXISTS project_controller_decisions_no_delete
BEFORE DELETE ON project_controller_decisions
BEGIN
    SELECT RAISE(ABORT, 'project_controller_decisions is append-only');
END;

CREATE TRIGGER IF NOT EXISTS project_controller_decisions_update_guard
BEFORE UPDATE ON project_controller_decisions
WHEN NEW.id IS NOT OLD.id
  OR NEW.project_id IS NOT OLD.project_id
  OR NEW.based_on_revision IS NOT OLD.based_on_revision
  OR NEW.human_epoch IS NOT OLD.human_epoch
  OR NEW.snapshot_sha256 IS NOT OLD.snapshot_sha256
  OR NEW.request_sha256 IS NOT OLD.request_sha256
  OR NEW.generation_id IS NOT OLD.generation_id
  OR NEW.mode IS NOT OLD.mode
  OR NEW.proposal_json IS NOT OLD.proposal_json
  OR NEW.validation IS NOT OLD.validation
  OR NEW.effect_json IS NOT OLD.effect_json
  OR NEW.progress_epoch IS NOT OLD.progress_epoch
  OR NEW.created_at IS NOT OLD.created_at
  OR OLD.effect_state <> 'pending'
  OR NEW.effect_state NOT IN ('applied', 'failed')
BEGIN
    SELECT RAISE(ABORT, 'project_controller_decisions: only pending -> applied|failed may change');
END;
