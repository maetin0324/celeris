-- ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-harness）と統合 2026-10-13:
-- 根拠表 routing_evidence（0074）を評価 runner の書く形に揃える。
-- * pass_boot_lower / pass_boot_upper: 問題単位の bootstrap 95% 区間（seed 42・2000 resamples）。
-- * low_confidence: k=1・部分集合の候補（優越を名乗れない）。
-- * wall_median_s / usd_list_median: 結果の無い cell（定価の無い source 等）は NULL。
-- SQLite は列の NOT NULL を ALTER で外せないので表を作り直す。既存行はそのまま移す。
CREATE TABLE routing_evidence_new (
    genre TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    harness_version TEXT NOT NULL,
    source_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    evidence_version TEXT NOT NULL,
    n INTEGER NOT NULL CHECK (n >= 0),
    pass INTEGER NOT NULL CHECK (pass >= 0 AND pass <= n),
    pass_lower REAL NOT NULL,
    pass_upper REAL NOT NULL,
    pass_boot_lower REAL NOT NULL,
    pass_boot_upper REAL NOT NULL,
    wall_median_s REAL,
    usd_list_median REAL,
    quota_per_run_json TEXT NOT NULL,
    timeout_rate REAL NOT NULL,
    low_confidence INTEGER NOT NULL CHECK (low_confidence IN (0, 1)),
    PRIMARY KEY (genre, harness_id, harness_version, source_id, model_id, evidence_version)
);
INSERT INTO routing_evidence_new
    (genre, harness_id, harness_version, source_id, model_id, evidence_version, n, pass,
     pass_lower, pass_upper, pass_boot_lower, pass_boot_upper, wall_median_s, usd_list_median,
     quota_per_run_json, timeout_rate, low_confidence)
SELECT genre, harness_id, harness_version, source_id, model_id, evidence_version, n, pass,
       pass_lower, pass_upper, pass_lower, pass_upper, wall_median_s, usd_list_median,
       quota_per_run_json, timeout_rate, 0
FROM routing_evidence;
DROP INDEX IF EXISTS idx_routing_evidence_latest;
DROP TABLE routing_evidence;
ALTER TABLE routing_evidence_new RENAME TO routing_evidence;
CREATE INDEX idx_routing_evidence_latest ON routing_evidence
    (genre, harness_id, evidence_version DESC);
