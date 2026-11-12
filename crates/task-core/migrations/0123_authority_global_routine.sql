-- ADR 2026-11-11-cos-standing-context D4: authority ledger に global の範囲と定型の代答の分類を足す。
--
-- `scope_kind = 'global'` は全案件に効く許可で、行の `project_id` と `scope_ref` は '*'。持てる分類は定型の代答
-- （`job_wait_extend`）だけ。SQLite は CHECK を変えられないので表を作り直す。行はそのまま写し（消さない）、
-- 追記専用の trigger と index を作り直す。
CREATE TABLE authority_grants_new (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('project', 'outcome', 'task', 'global')),
    scope_ref TEXT NOT NULL CHECK (length(scope_ref) > 0),
    action_class TEXT NOT NULL CHECK (action_class IN (
        'destructive', 'production', 'api_authorization', 'external_publish',
        'opaque_external_effect', 'launch_root', 'job_wait_extend')),
    source_kind TEXT NOT NULL CHECK (source_kind IN (
        'human_message', 'decision', 'approval', 'standing_rule', 'human_operation')),
    source_ref TEXT NOT NULL CHECK (length(source_ref) > 0),
    granted_by TEXT NOT NULL CHECK (granted_by LIKE 'human:%'),
    question_key TEXT,
    note TEXT NOT NULL DEFAULT '',
    expires_at TEXT NOT NULL,
    budget INTEGER NOT NULL CHECK (budget > 0),
    consumed INTEGER NOT NULL DEFAULT 0 CHECK (consumed >= 0 AND consumed <= budget),
    revoked_at TEXT,
    revoked_by TEXT,
    created_at TEXT NOT NULL,
    CHECK ((scope_kind = 'global') = (project_id = '*')),
    CHECK (scope_kind <> 'global' OR (scope_ref = '*' AND action_class = 'job_wait_extend'))
);
INSERT INTO authority_grants_new (id, project_id, scope_kind, scope_ref, action_class, source_kind,
    source_ref, granted_by, question_key, note, expires_at, budget, consumed, revoked_at, revoked_by,
    created_at)
    SELECT id, project_id, scope_kind, scope_ref, action_class, source_kind, source_ref, granted_by,
        question_key, note, expires_at, budget, consumed, revoked_at, revoked_by, created_at
    FROM authority_grants;
DROP TABLE authority_grants;
ALTER TABLE authority_grants_new RENAME TO authority_grants;
CREATE INDEX IF NOT EXISTS authority_grants_project
    ON authority_grants (project_id, action_class, created_at, id);

CREATE TRIGGER IF NOT EXISTS authority_grants_no_delete
BEFORE DELETE ON authority_grants
BEGIN
    SELECT RAISE(ABORT, 'authority_grants is append-only');
END;

CREATE TRIGGER IF NOT EXISTS authority_grants_update_guard
BEFORE UPDATE ON authority_grants
WHEN NEW.id IS NOT OLD.id
  OR NEW.project_id IS NOT OLD.project_id
  OR NEW.scope_kind IS NOT OLD.scope_kind
  OR NEW.scope_ref IS NOT OLD.scope_ref
  OR NEW.action_class IS NOT OLD.action_class
  OR NEW.source_kind IS NOT OLD.source_kind
  OR NEW.source_ref IS NOT OLD.source_ref
  OR NEW.granted_by IS NOT OLD.granted_by
  OR NEW.question_key IS NOT OLD.question_key
  OR NEW.note IS NOT OLD.note
  OR NEW.expires_at IS NOT OLD.expires_at
  OR NEW.budget IS NOT OLD.budget
  OR NEW.created_at IS NOT OLD.created_at
  OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NOT OLD.revoked_at)
BEGIN
    SELECT RAISE(ABORT, 'authority_grants: only consumed and the revocation may change');
END;
