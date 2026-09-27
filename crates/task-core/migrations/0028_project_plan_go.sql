-- Migration 28 (schema_version=28): ADR-0074 D3.2 / D3.8（Phase F4b）。
--
--   projects.auto_advance — 案件ごとの設定。1 なら、案件計画のマイルストーン Task の依存先が `done` に
--                           なった時点で、途中目標の `reached`（人の `ok`）を待たずに進める（既定 0）。
--   milestones.plan_key   — 案件計画（`celeris.project-plan/1` と差分）から作られた途中目標の key。
--                           NULL の途中目標（既存・手で作ったもの・`task_ops::add` の自動生成）は
--                           旧い意味（直列、ADR-0038 の `ok` で次を承認して分解）のまま（D3.8）。
--
-- 既存の行はどちらも既定値（0 / NULL）になり、挙動は変わらない。

ALTER TABLE projects ADD COLUMN auto_advance INTEGER NOT NULL DEFAULT 0;
ALTER TABLE milestones ADD COLUMN plan_key TEXT;
