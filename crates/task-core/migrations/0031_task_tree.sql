-- Migration 31 (schema_version=31): ADR-0079 D4 / D7 / D15（Phase R1a）。再帰的な task の木。
--
-- すべて events の派生（落としても replay で作り直せる）。**既存の行は書き換えない**（D13「migration は
-- 行を書き換えない。凍結」、D15「tree を持たない既存の task は深さ 1 の節点として扱う（root_id は NULL の
-- まま。埋め戻さない）」）。
--
--   tasks.root_id                  — `Task.tree.root_id` の写し（索引。正本は `json` の中の `tree`）。
--                                    木に属さない task は NULL。
--   work_units.child_task_id       — kind task の unit の子 task（`Event::ChildTaskCreated` /
--                                    `Event::ChildAdopted` の写し）。leaf・統合 WU は NULL。
--   work_units.needs_decisions_json — その unit が回答を待つ決定の key（/3 の計画の `needs_decisions` と
--                                    `needed_before`。`Event::ExecutionPlanned` から決まる）。/1・/2 は '[]'。
--   decisions                      — 人への決定の要求（`Event::DecisionRequested` / `DecisionAnswered` /
--                                    `DecisionWithdrawn` の畳み込み。`json` は DecisionRequest 全体）。
--
-- `SchemaTooNew` の規則どおり、旧いバイナリ（SCHEMA_VERSION 30）は版数 31 の DB を開けない
-- （昇格は stop → start。ロールバックは ADR-0040 D2）。

ALTER TABLE tasks ADD COLUMN root_id TEXT;
CREATE INDEX IF NOT EXISTS idx_tasks_root_id ON tasks (root_id) WHERE root_id IS NOT NULL;

ALTER TABLE work_units ADD COLUMN child_task_id TEXT;
ALTER TABLE work_units ADD COLUMN needs_decisions_json TEXT NOT NULL DEFAULT '[]';
CREATE INDEX IF NOT EXISTS idx_work_units_child_task ON work_units (child_task_id) WHERE child_task_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS decisions (
    id                 TEXT PRIMARY KEY,
    root_id            TEXT NOT NULL,
    task_id            TEXT NOT NULL,
    key                TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('choice','leaf_too_large','limit','plan_invalid')),
    status             TEXT NOT NULL CHECK (status IN ('open','answered','withdrawn')),
    needed_before_json TEXT NOT NULL DEFAULT '[]',
    json               TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    answered_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_decisions_root ON decisions (root_id, status);
CREATE INDEX IF NOT EXISTS idx_decisions_task ON decisions (task_id, status);
