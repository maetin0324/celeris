-- Migration 95（schema_version=95）: ADR 2026-10-27-project-brief D1/D2。
--
-- `project_briefs` — 案件ごとの状態の要約（目的・現状・未解決・次の手・待っている相手・根拠）。1 案件 1 行、
-- 書き込みは全置換（`PUT /projects/{id}/status`、CoS op `project.status_put`）。一覧の欄は JSON 配列。
-- `projects.secretary_summary` は書く経路が無いまま残っていたので消す。値が残っていた行は消す前に `state` へ写す。
CREATE TABLE project_briefs (
    project_id         TEXT PRIMARY KEY REFERENCES projects(id),
    objective          TEXT NOT NULL,
    state              TEXT NOT NULL,
    open_issues_json   TEXT NOT NULL DEFAULT '[]',
    next_actions_json  TEXT NOT NULL DEFAULT '[]',
    waiting_on_json    TEXT NOT NULL DEFAULT '[]',
    evidence_refs_json TEXT NOT NULL DEFAULT '[]',
    updated_at         TEXT NOT NULL,
    updated_by         TEXT NOT NULL
);

INSERT INTO project_briefs (project_id, objective, state, updated_at, updated_by)
SELECT id, request, secretary_summary, updated_at, 'migration:0095'
FROM projects
WHERE secretary_summary IS NOT NULL AND trim(secretary_summary) <> '';

ALTER TABLE projects DROP COLUMN secretary_summary;
