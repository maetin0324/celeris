-- Migration 98（schema_version=98）: ADR 2026-10-27-decision-log-intervention-actors D1。
--
-- `decision_log_interventions` に主体（`actor_kind`・`actor_id`）を足す（CHECK を付けるため表を作り直す）。
-- 既存の行は主体が分からないので `unknown`（`human` で埋めない）。行は events からの冪等な投影で、次の投影で
-- 主体付きに置き換わる。作り直しは何度流しても同じ結果になる。
CREATE TABLE decision_log_interventions_new (
    intervention_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('research_judgment', 'manual_handoff', 'recovery', 'unclassified')),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('human', 'cos_project', 'cos_general', 'external_operator', 'mcp_client', 'system', 'unknown')),
    actor_id TEXT,
    task_id TEXT,
    campaign_id TEXT,
    at TEXT NOT NULL,
    wall_ms INTEGER,
    source_event TEXT
);

INSERT INTO decision_log_interventions_new (intervention_id, kind, actor_kind, actor_id, task_id, campaign_id, at, wall_ms, source_event)
SELECT intervention_id, kind, 'unknown', NULL, task_id, campaign_id, at, wall_ms, source_event FROM decision_log_interventions;

DROP TABLE decision_log_interventions;
ALTER TABLE decision_log_interventions_new RENAME TO decision_log_interventions;
CREATE INDEX IF NOT EXISTS idx_decision_log_interventions_campaign ON decision_log_interventions (campaign_id, intervention_id);
