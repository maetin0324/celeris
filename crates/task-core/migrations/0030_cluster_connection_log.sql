-- Migration 30 (schema_version=30): ADR-0078 D5。
--
-- `cluster_connection_log` — クラスタの ssh master の接続・切断・鍵認証の再接続の試みを 1 行ずつ残す
-- （追記だけ）。`GET /clusters` の `stats`（直近 24 時間の回数）が数える。daemon の再起動をまたいで
-- 数えるために DB に置く（タスクに紐づかないので `events` には置かない）。
--
--   id           — 連番。
--   cluster_id   — `[[clusters]] id`。
--   kind         — `connected` | `lost` | `key_auth_attempt`。
--   method       — `connected` のとき `totp` | `publickey` | `borrowed`。他は NULL。
--   cause        — `lost` のとき `check_failed` | `probe_failed` | `master_exited` | `explicit`。
--                  `key_auth_attempt` のとき `ok` | `failed`。他は NULL。
--   uptime_secs  — `lost` のとき、接続していた秒数（分かれば）。
--   at           — RFC 3339。

CREATE TABLE IF NOT EXISTS cluster_connection_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    method TEXT,
    cause TEXT,
    uptime_secs INTEGER,
    at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cluster_connection_log_at ON cluster_connection_log (at);
