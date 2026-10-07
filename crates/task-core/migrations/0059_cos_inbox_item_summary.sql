-- ADR 2026-10-07-cos-inbox-thread-conversation D1: 件の題名を投入時に保存する。
-- 受信箱スレッドの判断の記録（digest）と人の発言の run への文脈に使う。既存行は空文字（digest は source で代用）。
ALTER TABLE cos_inbox_items ADD COLUMN summary TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_cos_inbox_items_run ON cos_inbox_items(run_id);
