-- Migration 90 (schema_version=90): config.toml の欄の変更の履歴（ADR 2026-10-22-dynamic-config D4）。
-- 追記専用（INSERT だけ。UPDATE・DELETE を持たない）。secret の値は書かない（欄の値は daemon が伏せてから渡す）。
CREATE TABLE config_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    key TEXT NOT NULL,
    class TEXT NOT NULL,
    apply_unit TEXT NOT NULL,
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL
);
