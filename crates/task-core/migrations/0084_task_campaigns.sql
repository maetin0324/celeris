-- Migration 84 (schema_version=84): campaign 識別（ADR 2026-10-16-campaign-identity-propagation）。
--
-- campaign の root task に付く識別（campaign_id / case / arm / repetition / manifest hash / routing 設定）。
-- 子孫 task は自分の行を持たず、parent_id の鎖を辿って最も近い祖先の行を継ぐ（読み出し時の解決。
-- 子 task の生成経路が増えても伝搬が落ちない）。campaign 無しの task は行を持たない。
CREATE TABLE IF NOT EXISTS task_campaigns (
    task_id TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL,
    case_id TEXT,
    arm TEXT,
    repetition INTEGER,
    manifest_hash TEXT,
    routing_setting TEXT CHECK (routing_setting IS NULL OR routing_setting IN ('fixed', 'decision_assisted')),
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_campaigns_campaign ON task_campaigns (campaign_id, task_id);

-- 決定ログの run 行にも同じ識別を落とす（arm / campaign_id は既存の列）。
ALTER TABLE decision_log_runs ADD COLUMN case_id TEXT;
ALTER TABLE decision_log_runs ADD COLUMN repetition INTEGER;
ALTER TABLE decision_log_runs ADD COLUMN manifest_hash TEXT;
ALTER TABLE decision_log_runs ADD COLUMN routing_setting TEXT;
