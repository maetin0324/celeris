-- ADR 2026-11-06-cos-chat-launch-scaling: queued 入力と実行中 run を全 thread 走査せずに引く。
CREATE INDEX IF NOT EXISTS idx_chat_messages_queued ON chat_messages(thread_id) WHERE state='queued';
CREATE INDEX IF NOT EXISTS idx_chat_runs_open ON chat_runs(thread_id) WHERE state IN ('running','stopping');
