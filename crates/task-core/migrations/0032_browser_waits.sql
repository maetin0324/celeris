-- Migration 31 (schema_version=31): ADR-0080 D3/D4/D5。
--
-- ブラウザの人待ち（登録依頼・承認）の耐久記録。**秘密（username/password・鍵・lease の中身・
-- dashboard token）はどの列にも置かない**。秘密は celeris-credentiald の暗号化 vault だけにある。
--
-- `browser_waits` — task を `blocked` にしている browser の wait 1 件。
--   wait_id               — ULID。
--   task_id / work_unit_id / run_id / session_id — 待った場所（WU が無ければ work_unit_id は NULL）。
--   reason                — `waiting_for_auth` | `waiting_for_approval`。
--   origin                — trusted exact HTTPS origin（`https://host[:port]`、既定 port は省く）。
--   purpose               — task 由来の短い説明（最大 500 文字、plain text、untrusted 表示）。
--   credential_id / credential_provider / credential_policy_id — credential の参照（秘密ではない）。
--                           登録待ちでは credential_policy_id だけが入り、登録後に id/provider が埋まる。
--   operation_intent_id / action / args_digest — 承認待ちの操作 intent（内部 action 名と非秘密の digest）。
--   approval_id           — 承認されたときの `browser_approvals.approval_id`。
--   policy_revision / policy_hash — 待った時点の task policy。
--   owner_id              — 本人（GUI owner）の識別子。未設定なら NULL。
--   deadline              — RFC 3339。登録待ちは既定・上限 24 時間、承認待ちは既定・上限 5 分。
--   resume_key            — 呼び出し側（supervisor）が決める一意の鍵。同じ鍵の再送は同じ wait を返す。
--   version               — CAS 用。状態が変わるたびに 1 増える。
--   state                 — `pending` | `registered` | `approved` | `resumed` | `denied` | `expired`
--                           | `cancelled` | `revoked` | `invalidated`。
--   resolution_code       — 固定コード（`registered` / `approved` / `approval_denied` /
--                           `browser_wait_expired` / `task_terminal` / `revoked` / `consumed`）。
CREATE TABLE IF NOT EXISTS browser_waits (
    wait_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    work_unit_id TEXT,
    run_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (reason IN ('waiting_for_auth', 'waiting_for_approval')),
    origin TEXT NOT NULL,
    purpose TEXT NOT NULL,
    credential_id TEXT,
    credential_provider TEXT,
    credential_policy_id TEXT,
    operation_intent_id TEXT,
    action TEXT,
    args_digest TEXT,
    approval_id TEXT,
    policy_revision INTEGER NOT NULL,
    policy_hash TEXT NOT NULL,
    owner_id TEXT,
    deadline TEXT NOT NULL,
    resume_key TEXT NOT NULL UNIQUE,
    version INTEGER NOT NULL,
    state TEXT NOT NULL,
    resolution_code TEXT,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
-- 1 task に未解決（pending / approved で未消費）の wait は高々 1 件。
CREATE UNIQUE INDEX IF NOT EXISTS idx_browser_waits_open_task
    ON browser_waits (task_id) WHERE state IN ('pending', 'approved');
CREATE INDEX IF NOT EXISTS idx_browser_waits_state_deadline ON browser_waits (state, deadline);

-- `browser_credentials` — credential の台帳（ADR-0080 D3: credential_id・provider・policy・origin だけ）。
--   receipt_id は broker が発行した登録 receipt（秘密ではない）。
CREATE TABLE IF NOT EXISTS browser_credentials (
    credential_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    policy_id TEXT NOT NULL,
    credential_revision INTEGER NOT NULL,
    origin TEXT NOT NULL,
    receipt_id TEXT NOT NULL,
    wait_id TEXT,
    registered_at TEXT NOT NULL
);

-- `browser_approvals` — 人の決定（approve_once / deny / revoke）の記録。
--   nonce は human attestation の一回限りの値（再送の検出）。idempotency_key は GUI の再送の冪等化。
--   consumed_at は approve_once の一回の実行権を消費した時刻。
CREATE TABLE IF NOT EXISTS browser_approvals (
    approval_id TEXT PRIMARY KEY,
    wait_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    decision TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    owner_session_hash TEXT NOT NULL,
    policy_hash TEXT NOT NULL,
    nonce TEXT NOT NULL UNIQUE,
    idempotency_key TEXT NOT NULL UNIQUE,
    decided_at TEXT NOT NULL,
    consumed_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_browser_approvals_wait ON browser_approvals (wait_id);
