-- Migration 85 (schema_version=85): 全消費量の計測（ADR 2026-10-16-metering-attribution）。
--
-- 消費行に帰属規則と共有オーバーヘッドの別記を足す。NULL の数値は欠測、0 は 0 と観測した値（既存の規約のまま）。
-- attribution_rule が NULL の行は campaign に直接帰属する（規則を要さない）。規則を使った行はその名前を残す。
-- shared_overhead=1 の行は特定の run・case に割れない共有処理（CoS の対話など）で、run 単位の合計に混ぜない。
ALTER TABLE decision_log_usage ADD COLUMN attribution_rule TEXT;
ALTER TABLE decision_log_usage ADD COLUMN shared_overhead INTEGER NOT NULL DEFAULT 0;

-- campaign の最後の活動時刻（壁時計の elapsed = これ − task_campaigns.created_at の最小）。
-- run 時間の総和（decision_log_usage.wall_ms の合計）とは別欄。再処理は MAX で畳むので増えない。
CREATE TABLE IF NOT EXISTS decision_log_campaign_activity (
    campaign_id TEXT PRIMARY KEY,
    last_activity_at TEXT NOT NULL
);
