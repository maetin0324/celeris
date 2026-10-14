-- Migration 79 (schema_version=79): ADR 2026-10-14-local-machine-settings D2。
--
-- ローカルマシンの上書き（1 行だけ。id = 1）。NULL の欄は設定ファイルの値を使う。
CREATE TABLE IF NOT EXISTS local_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    workspace_root TEXT,          -- 正規化済みの絶対 path（`~` は PUT 時に展開して保存）
    max_concurrency INTEGER,      -- 1..=256
    updated_at TEXT NOT NULL
);
-- task ごとの足回りの root の固定（D5）。INSERT だけ、更新しない。
CREATE TABLE IF NOT EXISTS task_workspace_roots (
    task_id TEXT PRIMARY KEY,
    root TEXT NOT NULL,
    pinned_at TEXT NOT NULL
);
-- 変更の監査（追記専用。org_browser_events と同じく task を持たない流れ）。
CREATE TABLE IF NOT EXISTS local_settings_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,          -- "admin" | "cos:<operation id>"
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL
);
