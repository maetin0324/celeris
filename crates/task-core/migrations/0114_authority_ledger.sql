-- ADR 2026-11-04-project-controller A7: 人の許可範囲（authority ledger）。
--
-- `authority_grants`: 人が与えた許可 1 件。期限・予算・元の根拠は必須で、無条件の許可は表に入らない。
--   `human_required` は許可で解除できないので CHECK で弾く。行は消せず、`consumed` の増減と取り消しの欄だけが変わる。
-- `authority_audit`: 許可の判定と実行の監査（suggestion・observed outcome・actual actor・human decision）。
--   行は消せず、`outcome_state = 'unknown'` の行の結果の欄だけが 1 回だけ変わる。
-- `authority_proposals`: CoS が人の発言を引いて出す許可の提案。人が許可するか断るまで open。
-- 状態の行が無い案件・`off` の案件ではこの表を読まない（I13）。
CREATE TABLE IF NOT EXISTS authority_grants (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('project', 'outcome', 'task')),
    scope_ref TEXT NOT NULL CHECK (length(scope_ref) > 0),
    action_class TEXT NOT NULL CHECK (action_class IN (
        'destructive', 'production', 'api_authorization', 'external_publish',
        'opaque_external_effect', 'launch_root')),
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
    created_at TEXT NOT NULL
);
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

CREATE TABLE IF NOT EXISTS authority_audit (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    -- 判定に渡された分類（不明な分類の試みも残すので CHECK を付けない）。
    action_class TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    grant_id TEXT,
    allow INTEGER NOT NULL CHECK (allow IN (0, 1)),
    reason TEXT NOT NULL,
    suggestion_json TEXT NOT NULL,
    actual_actor TEXT NOT NULL,
    human_decision_ref TEXT,
    decision_id TEXT,
    -- 同じ操作の記録を 1 行にする key（`cos:<thread>:<idempotency>`、`pc-deny:<project>:<question>` など）。
    dedupe_key TEXT NOT NULL UNIQUE,
    outcome_state TEXT NOT NULL CHECK (outcome_state IN ('unknown', 'applied', 'failed', 'refused')),
    observed_outcome_json TEXT,
    created_at TEXT NOT NULL,
    observed_at TEXT
);
CREATE INDEX IF NOT EXISTS authority_audit_project
    ON authority_audit (project_id, created_at, id);
CREATE INDEX IF NOT EXISTS authority_audit_unknown
    ON authority_audit (project_id, action_class) WHERE outcome_state = 'unknown';

CREATE TRIGGER IF NOT EXISTS authority_audit_no_delete
BEFORE DELETE ON authority_audit
BEGIN
    SELECT RAISE(ABORT, 'authority_audit is append-only');
END;

CREATE TRIGGER IF NOT EXISTS authority_audit_update_guard
BEFORE UPDATE ON authority_audit
WHEN OLD.outcome_state <> 'unknown'
  OR NEW.id IS NOT OLD.id
  OR NEW.project_id IS NOT OLD.project_id
  OR NEW.action_class IS NOT OLD.action_class
  OR NEW.scope_kind IS NOT OLD.scope_kind
  OR NEW.scope_ref IS NOT OLD.scope_ref
  OR NEW.grant_id IS NOT OLD.grant_id
  OR NEW.allow IS NOT OLD.allow
  OR NEW.reason IS NOT OLD.reason
  OR NEW.suggestion_json IS NOT OLD.suggestion_json
  OR NEW.actual_actor IS NOT OLD.actual_actor
  OR NEW.human_decision_ref IS NOT OLD.human_decision_ref
  OR NEW.decision_id IS NOT OLD.decision_id
  OR NEW.dedupe_key IS NOT OLD.dedupe_key
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'authority_audit: only an unknown outcome may be settled, once');
END;

CREATE TABLE IF NOT EXISTS authority_proposals (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    question_key TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_ref TEXT NOT NULL,
    action_class TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    budget INTEGER NOT NULL CHECK (budget > 0),
    note TEXT NOT NULL DEFAULT '',
    proposed_by TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'accepted', 'dismissed')),
    resolved_grant_id TEXT,
    resolved_by TEXT,
    resolved_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (project_id, question_key)
);

CREATE TRIGGER IF NOT EXISTS authority_proposals_no_delete
BEFORE DELETE ON authority_proposals
BEGIN
    SELECT RAISE(ABORT, 'authority_proposals is append-only');
END;
