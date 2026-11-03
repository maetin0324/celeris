-- ADR 2026-11-03-ts-intended-behavior-change D4: 人が承認した TS の方針改善（behavior change）の仕様と、その記録。
--
-- `script_behavior_changes` は仕様 1 版（hash）につき 1 行の現在の状態。`script_behavior_change_events` は承認・取り消し・
-- 版の置き換えの追記専用の記録（store に UPDATE・DELETE の経路を作らない）。dispatcher は `approved` の行だけを比較の説明に使う。
CREATE TABLE IF NOT EXISTS script_behavior_changes (
    spec_sha256 TEXT PRIMARY KEY CHECK (length(spec_sha256) = 64),
    change_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 1),
    domain TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('approved', 'revoked', 'superseded')),
    approved_by TEXT NOT NULL,
    approved_at TEXT NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (change_id, version)
);
CREATE INDEX IF NOT EXISTS script_behavior_changes_state ON script_behavior_changes (domain, state);

CREATE TABLE IF NOT EXISTS script_behavior_change_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    spec_sha256 TEXT NOT NULL,
    change_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('approved', 'revoked', 'superseded')),
    actor TEXT NOT NULL,
    reason TEXT NOT NULL
);
