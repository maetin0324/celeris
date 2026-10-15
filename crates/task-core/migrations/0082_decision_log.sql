-- Migration 81 (schema_version=81): ADR 2026-10-15-decision-log-export D1〜D7。
--
-- 判断・実行・成果・消費の最小一貫ログ。新しい保管系ではなく、既存の routing_decided / RunMetrics /
-- route_decisions を読み元に、足りない欄だけを run_id 単位で持つ。events は触らない（追記専用のまま）。
-- 提案（proposed_*）・実行（exec_*）・成果（run_end / wu_verdict / campaign_verdict）は欄の群が別で、
-- 群ごとの upsert が他の群を上書きしない。再処理は主キー（run_id / dedup_key / intervention_id）の置換で
-- 合算しない。classifier_confidence と propensity は別の欄（互いの代入・流用をしない）。
CREATE TABLE IF NOT EXISTS decision_log_runs (
    run_id TEXT PRIMARY KEY,
    -- 鍵（D1）
    decision_id TEXT,
    campaign_id TEXT,
    arm TEXT,
    project_id TEXT,
    task_id TEXT,
    work_unit_id TEXT,
    parent_run_id TEXT,
    attempt_no INTEGER,
    -- 提案（D2）
    request_class TEXT,
    routing_mode TEXT CHECK (routing_mode IS NULL OR routing_mode IN ('fixed', 'decision_assisted')),
    classifier_kind TEXT CHECK (classifier_kind IS NULL OR classifier_kind IN ('rule', 'jev', 'human_hint')),
    classifier_model TEXT,
    classifier_snapshot TEXT,
    classifier_confidence REAL CHECK (classifier_confidence IS NULL OR (classifier_confidence >= 0.0 AND classifier_confidence <= 1.0)),
    classifier_probs_json TEXT,
    propensity REAL,
    proposed_lane TEXT,
    decided_lane TEXT,
    proposed_candidates_json TEXT,
    proposed_first_source_id TEXT,
    proposed_first_model TEXT,
    proposed_first_harness TEXT,
    exclusions_json TEXT,
    policy_version TEXT,
    prompt_version TEXT,
    question_schema_version TEXT,
    provider_model_version TEXT,
    features_hash TEXT,
    options_hash TEXT,
    -- 実行（D2）
    exec_source_id TEXT,
    exec_model TEXT,
    exec_account TEXT,
    exec_harness TEXT,
    exec_adapter TEXT,
    fallback_reason TEXT,
    classifier_fallback TEXT,
    attempts_json TEXT,
    -- 成果（D3）: 3 階層は別欄。未判定は NULL
    run_end TEXT CHECK (run_end IS NULL OR run_end IN
        ('completed', 'yielded', 'budget_exhausted', 'question', 'failed', 'harness_error', 'cancelled', 'waiting', 'timeout')),
    run_end_raw TEXT,
    run_retryable INTEGER,
    wu_verdict TEXT CHECK (wu_verdict IS NULL OR wu_verdict IN ('passed', 'failed', 'unverified')),
    wu_failure_class TEXT,
    campaign_verdict TEXT CHECK (campaign_verdict IS NULL OR campaign_verdict IN ('pass', 'fail', 'unscored')),
    wall_ms INTEGER,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decision_log_runs_campaign ON decision_log_runs (campaign_id, run_id);
CREATE INDEX IF NOT EXISTS idx_decision_log_runs_exec ON decision_log_runs (exec_source_id, exec_model);
CREATE INDEX IF NOT EXISTS idx_decision_log_runs_decision ON decision_log_runs (decision_id);

-- 消費（D5）。行の主キーは dedup_key（run 由来 `run:<run_id>:<source>`、proxy 外の呼び出し `decision:<id>:<attempt>`）。
-- 数値欄の NULL は欠測、0 は「0 と観測した」。推定値は *_estimated を併記する。
CREATE TABLE IF NOT EXISTS decision_log_usage (
    dedup_key TEXT PRIMARY KEY,
    run_id TEXT,
    decision_id TEXT,
    campaign_id TEXT,
    task_id TEXT,
    role TEXT NOT NULL CHECK (role IN ('cos', 'planner', 'worker', 'reviewer', 'jev')),
    is_retry INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_creation_tokens INTEGER,
    list_cost_usd REAL,
    list_cost_usd_estimated INTEGER NOT NULL DEFAULT 1,
    cash_cost_usd REAL,
    wall_ms INTEGER,
    cluster_wait_ms INTEGER,
    cluster_run_ms INTEGER,
    cluster_node_hours REAL,
    cluster_queue_ms INTEGER,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decision_log_usage_campaign ON decision_log_usage (campaign_id, dedup_key);

-- 人の介入（D7）。intervention_id は元 event の seq 由来で冪等。
CREATE TABLE IF NOT EXISTS decision_log_interventions (
    intervention_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('research_judgment', 'manual_handoff', 'recovery', 'unclassified')),
    task_id TEXT,
    campaign_id TEXT,
    at TEXT NOT NULL,
    wall_ms INTEGER,
    source_event TEXT
);
CREATE INDEX IF NOT EXISTS idx_decision_log_interventions_campaign ON decision_log_interventions (campaign_id, intervention_id);
