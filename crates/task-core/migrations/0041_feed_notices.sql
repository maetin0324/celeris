-- ADR-0133 D3.2: 通知（アプリ内の知らせ）。既読と束ねを持つ。
-- `notifications`（migration 0008/0009/0019）は外部への送り出しの待ち行列のまま残し、ここでは触らない。
-- 0038 / 0039 / 0040 は他の celeris/* ブランチ（work_unit_sessions・cron_jobs・write_sets・behind_targets）が使うので 0041。
CREATE TABLE IF NOT EXISTS feed_notices (
  id          TEXT PRIMARY KEY,          -- ULID
  kind        TEXT NOT NULL,
  group_key   TEXT NOT NULL,
  title       TEXT NOT NULL,
  summary     TEXT NOT NULL,
  project_id  TEXT,
  task_id     TEXT,
  target_kind TEXT,                      -- 対象の種類（task / report / release / delivery / cron_job …）
  target_id   TEXT,                      -- 対象の id（最新の 1 件）
  links_json  TEXT NOT NULL DEFAULT '[]',
  count       INTEGER NOT NULL DEFAULT 1,
  first_at    TEXT NOT NULL,
  last_at     TEXT NOT NULL,
  read_at     TEXT
);
-- 未読の束は group_key ごとに高々 1 行。
CREATE UNIQUE INDEX IF NOT EXISTS feed_notices_open_group ON feed_notices(group_key) WHERE read_at IS NULL;
CREATE INDEX IF NOT EXISTS feed_notices_unread ON feed_notices(read_at, last_at DESC);
-- 冪等: 1 出来事は 1 回だけ数える。
CREATE TABLE IF NOT EXISTS feed_sources (
  source_key  TEXT PRIMARY KEY,           -- 例 'event:<seq>' / 'report:<id>' / 'cron_run:<id>'
  notice_id   TEXT NOT NULL REFERENCES feed_notices(id) ON DELETE CASCADE,
  recorded_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS feed_sources_notice ON feed_sources(notice_id);
-- 走査位置（events の seq 等）。
CREATE TABLE IF NOT EXISTS feed_cursor (name TEXT PRIMARY KEY, value TEXT NOT NULL);
