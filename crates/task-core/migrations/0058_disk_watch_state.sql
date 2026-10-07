-- ADR 2026-10-07-build-tmp-hygiene D4.4: ディスク使用率の監視の状態（path ごとに 1 行）。
-- level は ok | warn | critical | unavailable。通知・受信箱は level が上がったときだけ作る（daemon のメモリに持たない）。
CREATE TABLE IF NOT EXISTS disk_watch_state (
    path TEXT PRIMARY KEY,
    level TEXT NOT NULL,
    since TEXT NOT NULL,
    last_pct REAL,
    last_notified_at TEXT,
    updated_at TEXT NOT NULL
);
