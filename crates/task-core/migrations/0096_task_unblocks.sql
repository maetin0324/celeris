-- Migration 96（schema_version=96）: ADR 2026-10-27-unblocks-reopen-on-promotion D1・D4。
--
-- `task_unblocks` — 修正 task（`fix_task_id`）と、その修正を待っている task（`waiting_task_id`）の結び。
-- 修正 task の作成（`POST /tasks` の `unblocks`）と同じ transaction で書く。修正 task を含む release の昇格を
-- daemon が見つけたら `fired_*` を 1 度だけ書く（`fired_at IS NULL` の行だけが対象）。
CREATE TABLE IF NOT EXISTS task_unblocks (
    fix_task_id TEXT NOT NULL,
    waiting_task_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    fired_release TEXT,
    fired_action TEXT,
    fired_at TEXT,
    PRIMARY KEY (fix_task_id, waiting_task_id)
);
CREATE INDEX IF NOT EXISTS task_unblocks_pending ON task_unblocks (fix_task_id) WHERE fired_at IS NULL;
