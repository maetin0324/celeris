---
title: Phase 5 core-model — write-set hint の保存先（ADR-0130 D1 の実装上の差分）
tasks: [01M3YE0JTQEYBFDTV3HCHR4G9J]
status: done
updated: 2026-10-03
---
# Phase 5 core-model: write-set hint の保存先（ADR-0130 D1 の実装上の差分）

- `expected_write_paths` は `celeris.execution-plan/3` の `PlanUnitSpec` だけが持つ任意欄にした。`Task` と v1/v2 の `WorkUnitSpec` の struct には足していない（全 crate の struct literal、特に crates/celeris・celerisctl を変えないため）。
- task の明示 hint は migration 0039 の `task_write_hints` 表に置く（`SqliteStore::set_task_expected_write_paths` / `task_expected_write_paths`。空配列 = 消去 = 未指定、不正形式は `StoreError::Invalid`）。
- 継承は読み出し時に導出する: `work_unit_expected_write_paths` = 計画の unit の値 → 無ければ task の有効値。`effective_task_write_paths` = 自分の明示値 → 無ければ `work_units.child_task_id` で親 unit の有効値。replan 後は WU の `plan_id` の計画を読む。
- 実績は `run_write_sets` / `work_unit_write_sets`（SHA 不変、complete を incomplete で上書きしない）。
- v1/v2 の WorkUnitSpec 欄と task 作成 API での指定は api-writeset 葉（または verify 葉で ADR に追記）で扱う。
- 着手時 main = 7b77f17a（変化なし）。`git merge-tree --write-tree --name-only HEAD main` は exit 1、衝突候補 9 件（task-api query.rs と tests、task-core cluster_job/tests.rs・store/migrations.rs・store/tests.rs、task-ops delivery.rs と tests、docs/PROGRESS.md、gui inbox.tsx）。main の migration は 0037 まで、他の celeris/* に 0039 は無い。
