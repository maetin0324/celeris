-- ADR 2026-10-08-browser-prod-enablement D3: 管理者の site policy（ログインの origin・URL・selector）の正本。
-- 秘密は持たない。source = 'config' は config.toml の [[api.browser_site_policies]] から空 DB に入れた種、
-- 'api' は API で書いたもの。時刻は RFC3339。
CREATE TABLE IF NOT EXISTS browser_site_policies (
    policy_id TEXT PRIMARY KEY,
    exact_origin TEXT NOT NULL,
    login_url TEXT NOT NULL,
    password_selector TEXT NOT NULL,
    submit_selector TEXT,
    source TEXT NOT NULL CHECK (source IN ('api', 'config')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
-- site policy には task_id が無い。変更の監査は org_browser_events（0051）と同じく別の追記専用の流れに置く。
-- selector・URL は載せない（値は表にだけある）。
CREATE TABLE IF NOT EXISTS browser_site_policy_events (
    id INTEGER PRIMARY KEY,
    policy_id TEXT NOT NULL,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN ('upsert', 'delete')),
    source TEXT NOT NULL CHECK (source IN ('api', 'config'))
);
CREATE INDEX IF NOT EXISTS idx_browser_site_policy_events_policy
    ON browser_site_policy_events(policy_id, id);
