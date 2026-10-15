-- Migration 80 (schema_version=80): ADR 2026-10-15-root-done-cos-summary D1/D2/D7。
--
-- root task の done 遷移ごとに 1 行（task_id + done 遷移の seq）。CoS にまとめを書かせる専用スレッドと、
-- その状態（new → queued → notified | fallback）を持つ。再起動しても行から続きを再開する。
CREATE TABLE IF NOT EXISTS root_done_summaries (
    task_id TEXT NOT NULL,
    done_seq INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('new', 'queued', 'notified', 'fallback')),
    thread_id TEXT,
    input_message_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (task_id, done_seq)
);
CREATE INDEX IF NOT EXISTS idx_root_done_summaries_open ON root_done_summaries (state, created_at);

-- 旧 `task_ready`（root 完了の通知）は `root_done_summary` に置き換わった。行は失わず種別だけ移す
-- （key は同じ `<task id>:<seq>` の形）。未送信のまま残っていた行は送らず畳む。
UPDATE notifications SET ok = 0, error = COALESCE(error, 'retired: task_ready was replaced by root_done_summary')
 WHERE kind = 'task_ready' AND ok IS NULL;
UPDATE notifications SET kind = 'root_done_summary' WHERE kind = 'task_ready';
