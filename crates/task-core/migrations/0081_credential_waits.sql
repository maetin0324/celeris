-- Migration 81 (schema_version=81): ADR 2026-10-15-mlsys-min-decision-assisted D4。
--
-- credential 待ち。外部 provider の鍵が未登録で止まった work unit ごとに 1 行。**秘密の値は持たない**
-- （参照名 `secret_id` と fingerprint だけ）。状態は waiting → verified → resumed の一方向と、
-- 誤キー/権限不足の rejected（再登録待ち）、cancel 済みの cancelled。SQLite が真実で、再起動後はこの表から復元する。
CREATE TABLE IF NOT EXISTS credential_waits (
    id INTEGER PRIMARY KEY,
    work_unit_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    secret_id TEXT NOT NULL,             -- credential 参照名（例 jev_api_key）。値ではない
    provider TEXT NOT NULL,              -- 例 jev
    purpose TEXT NOT NULL,               -- 用途（人向けの短文。値を含めない）
    state TEXT NOT NULL CHECK (state IN ('waiting', 'rejected', 'verified', 'resumed', 'cancelled')),
    aggregation_key TEXT NOT NULL,       -- 同じ秘密を待つ wait は同じ値（`credential:<secret_id>`）
    notice_item_id TEXT,                 -- 集約した通知（受信箱 item）の id
    seen_fingerprint TEXT,               -- 最後に見た登録の fingerprint（reconcile 用。秘密ではない）
    last_verify_kind TEXT CHECK (last_verify_kind IN ('valid', 'invalid_key', 'insufficient_permission')),
    resumed_at TEXT,                     -- 再開を claim した時刻（resume 済み印）
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
-- 開いている wait は (work_unit, secret) につき 1 つだけ（enter_wait の冪等性）。終端済みの行は残す。
CREATE UNIQUE INDEX IF NOT EXISTS idx_credential_waits_open
    ON credential_waits (work_unit_id, secret_id) WHERE state IN ('waiting', 'rejected', 'verified');
CREATE INDEX IF NOT EXISTS idx_credential_waits_key ON credential_waits (aggregation_key, state);
CREATE INDEX IF NOT EXISTS idx_credential_waits_task ON credential_waits (task_id);

-- 秘密の保存・更新の完了（`secret_stored`。secret id と fingerprint だけ）。追記専用。
CREATE TABLE IF NOT EXISTS credential_wait_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    kind TEXT NOT NULL,                  -- 'secret_stored'
    secret_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_credential_wait_events_secret ON credential_wait_events (secret_id, id);
