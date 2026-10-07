-- ADR 2026-10-06 D4: 利用可能モデルの catalog（自動発見）・人の上書き（別表）・source ごとの最終発見記録。
-- 上書きは別表にして、自動更新（apply）は触らない。
CREATE TABLE IF NOT EXISTS model_catalog (
    source TEXT NOT NULL,
    model_id TEXT NOT NULL,
    display_name TEXT,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    available INTEGER NOT NULL,
    capabilities TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (source, model_id)
);
CREATE TABLE IF NOT EXISTS model_catalog_overrides (
    source TEXT NOT NULL,
    model_id TEXT NOT NULL,
    disabled INTEGER NOT NULL DEFAULT 0,
    tier TEXT,
    alias TEXT,
    note TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source, model_id)
);
CREATE TABLE IF NOT EXISTS model_catalog_discovery (
    source TEXT PRIMARY KEY,
    at INTEGER NOT NULL,
    ok INTEGER NOT NULL,
    error TEXT,
    count INTEGER NOT NULL
);
