-- Migration 90 (schema_version=90): 完了単位の根拠表と harness 残量切替の guardrail
-- （ADR 2026-10-22-harness-balance-and-lane-first-routing D2/D5/D6）。
--
-- 1. routing_evidence の鍵に lane を足し、run 単位の中央値（wall_median_s・usd_list_median）を
--    完了単位の指標（wall_p90_s・usd_per_pass・quota_cost_per_pass）に置き換える。旧い行は lane '' のまま
--    残す（データは消さない）。選択器は lane が一致する行だけを読むので、旧い行は選択に効かない。
-- 2. completion_outcomes: task / WorkUnit の終端（done / failed）ごとに 1 行。冪等な upsert（unit_kind, unit_id）。
-- 3. harness_balance_trips: guardrail が止めた実行クラス。人の reset だけが戻す。
CREATE TABLE routing_evidence_new (
    genre TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    harness_version TEXT NOT NULL,
    lane TEXT NOT NULL DEFAULT '',
    source_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    evidence_version TEXT NOT NULL,
    n INTEGER NOT NULL CHECK (n >= 0),
    pass INTEGER NOT NULL CHECK (pass >= 0 AND pass <= n),
    pass_lower REAL NOT NULL,
    pass_upper REAL NOT NULL,
    pass_boot_lower REAL NOT NULL,
    pass_boot_upper REAL NOT NULL,
    wall_p90_s REAL,
    usd_per_pass REAL,
    quota_cost_per_pass REAL,
    quota_per_run_json TEXT NOT NULL,
    timeout_rate REAL NOT NULL,
    low_confidence INTEGER NOT NULL CHECK (low_confidence IN (0, 1)),
    PRIMARY KEY (genre, harness_id, harness_version, lane, source_id, model_id, evidence_version)
);
INSERT INTO routing_evidence_new
    (genre, harness_id, harness_version, lane, source_id, model_id, evidence_version, n, pass,
     pass_lower, pass_upper, pass_boot_lower, pass_boot_upper, wall_p90_s, usd_per_pass,
     quota_cost_per_pass, quota_per_run_json, timeout_rate, low_confidence)
SELECT genre, harness_id, harness_version, '', source_id, model_id, evidence_version, n, pass,
       pass_lower, pass_upper, pass_boot_lower, pass_boot_upper, wall_median_s, usd_list_median,
       NULL, quota_per_run_json, timeout_rate, low_confidence
FROM routing_evidence;
DROP INDEX IF EXISTS idx_routing_evidence_latest;
DROP TABLE routing_evidence;
ALTER TABLE routing_evidence_new RENAME TO routing_evidence;
CREATE INDEX idx_routing_evidence_latest ON routing_evidence
    (genre, harness_id, evidence_version DESC);

CREATE TABLE completion_outcomes (
    unit_kind TEXT NOT NULL CHECK (unit_kind IN ('task', 'work_unit')),
    unit_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    -- 評価に使う葉の単位か（WU、または WU を持たない task）。WU を持つ task の行は 0。
    leaf INTEGER NOT NULL CHECK (leaf IN (0, 1)),
    genre TEXT NOT NULL,
    passed INTEGER NOT NULL CHECK (passed IN (0, 1)),
    harness_id TEXT,
    harness_version TEXT,
    source_id TEXT,
    model_id TEXT,
    lane TEXT,
    -- Jev の判定（lane_assist）。無ければ NULL。
    jev_specificity TEXT,
    jev_confidence REAL,
    jev_lane TEXT,
    classifier_status TEXT,
    propensity REAL,
    tokens_in INTEGER,
    tokens_out INTEGER,
    tokens_cache_read INTEGER,
    tokens_cache_creation INTEGER,
    usd_list REAL,
    quota_json TEXT NOT NULL DEFAULT '{}',
    wall_s REAL,
    attempts INTEGER NOT NULL,
    check_failures INTEGER NOT NULL,
    review_rejections INTEGER NOT NULL,
    replans INTEGER NOT NULL,
    timed_out INTEGER NOT NULL CHECK (timed_out IN (0, 1)),
    -- この単位の run で harness の並べ替えが先頭を変えた実行クラス（JSON 配列）。
    switched_classes_json TEXT NOT NULL DEFAULT '[]',
    celeris_version TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    PRIMARY KEY (unit_kind, unit_id)
);
CREATE INDEX idx_completion_outcomes_leaf ON completion_outcomes (leaf, completed_at);

CREATE TABLE harness_balance_trips (
    execution_class TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    tripped_at TEXT NOT NULL
);
