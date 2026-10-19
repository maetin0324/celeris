-- Migration 87 (schema_version=87): 分類段・lane 各段の記録（Jev 実分類の版・lane・call ID を通常記録へ）。
--
-- 全て NULL 可の追加列。NULL は「記録が無い／観測できなかった」で、設定値・0・空文字で埋めない。
-- 既存行（旧 event・旧 DB）は NULL のまま読める。classifier_model は応答が名乗った model 名、
-- classifier_snapshot は応答と独立に観測した版（既存列。意味の別は docs/ops の記録仕様）。
ALTER TABLE decision_log_runs ADD COLUMN classifier_status TEXT;
ALTER TABLE decision_log_runs ADD COLUMN classifier_call_id TEXT;
ALTER TABLE decision_log_runs ADD COLUMN fixed_lane TEXT;
ALTER TABLE decision_log_runs ADD COLUMN jev_lane TEXT;
ALTER TABLE decision_log_runs ADD COLUMN constrained_lane TEXT;
ALTER TABLE decision_log_runs ADD COLUMN lane_basis TEXT;
