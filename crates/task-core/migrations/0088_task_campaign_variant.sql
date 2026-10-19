-- Migration 88 (schema_version=88): campaign 識別の variant（専門分担アブレーションの比較 variant）。
--
-- `task_campaigns` に variant 参照（`CampaignVariantRef` の JSON）と検証済み構成 pin（`CampaignOrgPin`
-- の JSON）を足す。2 列は対で NULL か対で値を持つ。従来の行は両方 NULL のまま（variant 無しの試行）。
-- 決定ログの run 行には arm の分割用に variant id と内容 hash だけを落とす。
-- 87 は別 branch（decision_log_lane_stages）が使っているため飛ばす（RESERVED_VERSIONS）。
ALTER TABLE task_campaigns ADD COLUMN variant_json TEXT;
ALTER TABLE task_campaigns ADD COLUMN variant_org_json TEXT;

ALTER TABLE decision_log_runs ADD COLUMN variant_id TEXT;
ALTER TABLE decision_log_runs ADD COLUMN variant_content_sha256 TEXT;
