-- ADR 2026-10-07-browser-trusted-devices D2: ブラウザの信頼できる端末。秘密の値は持たず SHA-256 hex だけ。
-- 時刻は UNIX 秒。absolute_expires_at は NULL = 絶対上限なし（人の決定 device-policy）。
CREATE TABLE IF NOT EXISTS browser_trusted_devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    method TEXT NOT NULL DEFAULT 'cookie',
    secret_hash TEXT NOT NULL,
    prev_secret_hash TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at INTEGER NOT NULL,
    absolute_expires_at INTEGER,
    revoked_at INTEGER,
    revoked_reason TEXT,
    actor TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_browser_trusted_devices_secret
    ON browser_trusted_devices(secret_hash);
