-- ADR-0131 D1: 汎用の定期実行（cron job）と実行履歴。
--
-- cron_jobs: job 1 件 = 1 行。発火は daemon の tick（task_ops::cron）が `next_fire_at <= now` を見て行う。
--   schedule      — 5 欄の cron 式（task_core::cron::CronSchedule）。
--   timezone      — IANA 名。
--   overlap       — 'skip' | 'queue'（D2）。
--   catch_up      — 'latest' | 'skip'（D3）。
--   template_json — task_core::cron::CronTaskTemplate。
--   next_fire_at  — UTC RFC3339。enabled = 0 のとき NULL。
-- cron_job_runs: 発火 1 回 = 1 行（作った・溜めた・飛ばした・失敗）。
--   UNIQUE (job_id, scheduled_for, trigger) が二重発火の最後の砦（再起動・手動と定時の衝突）。

CREATE TABLE IF NOT EXISTS cron_jobs (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    enabled       INTEGER NOT NULL,
    schedule      TEXT NOT NULL,
    timezone      TEXT NOT NULL,
    overlap       TEXT NOT NULL CHECK (overlap IN ('skip', 'queue')),
    catch_up      TEXT NOT NULL CHECK (catch_up IN ('latest', 'skip')),
    template_json TEXT NOT NULL,
    next_fire_at  TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cron_jobs_due ON cron_jobs (enabled, next_fire_at);

CREATE TABLE IF NOT EXISTS cron_job_runs (
    id            TEXT PRIMARY KEY,
    job_id        TEXT NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
    scheduled_for TEXT NOT NULL,
    trigger       TEXT NOT NULL CHECK (trigger IN ('schedule', 'catch_up', 'manual')),
    outcome       TEXT NOT NULL
        CHECK (outcome IN ('created', 'queued', 'skipped_overlap', 'skipped_missed', 'error')),
    task_id       TEXT,
    detail        TEXT,
    recorded_at   TEXT NOT NULL,
    UNIQUE (job_id, scheduled_for, trigger)
);

CREATE INDEX IF NOT EXISTS cron_job_runs_job ON cron_job_runs (job_id, recorded_at DESC);
