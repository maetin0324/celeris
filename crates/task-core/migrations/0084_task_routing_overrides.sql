-- Migration 84 (schema_version=84): ADR 2026-10-16-mlsys-measure-task-routing D2。
--
-- 試行ごとの routing 設定（mode・下段・根拠表・オンライン更新）を task 単位で固定する。
-- `POST /tasks` の `routing_override` を task の作成と同じ transaction で 1 行書き、以後は変えない。
-- 行の無い task は従来どおり config の `[routing] mode` に従う（既存の DB では何も起きない）。
CREATE TABLE task_routing_overrides (
    task_id TEXT PRIMARY KEY,
    override_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
