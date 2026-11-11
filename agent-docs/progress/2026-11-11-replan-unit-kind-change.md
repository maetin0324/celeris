---
title: replan で unit の kind を task と leaf の間で変えたときの行の kind（child_create_failed の修正）
tasks: [01M74VC34XB4RPAB61AV4226YS]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# replan で unit の kind を task と leaf の間で変えたときの行の kind（child_create_failed の修正）

完了日: 2026-11-11

設計判断は [ADR 2026-11-11 replan の unit の kind の変更](../adr/2026-11-11-replan-unit-kind-change.md)。基準は `git merge-base HEAD main`。

## 原因（行の kind と計画の kind が食い違う段）

本番の健全性 v2 R4（task 01M6TYVJT4S29ZMP6P0D4831JG）の unit change-map で起きた順に書く。

1. 計画 v1 の採用（`execution_plan_adopt_tree`）で、change-map は `kind = task` の行として作られた。root の計画は承認待ち
   （ADR-0079 D8）なので子 task は作られなかった。
2. 人が replan に回し、planner の v2 が change-map を leaf に書き直した。採用の `task_ops::execution::plan_replan` は既存の行を
   写して `spec`・`depends_on`・`phase`・`seq` を v2 に書き換えたが、**`kind` を書き換えなかった**（食い違いが生まれる段）。
   store の `update_work_unit_tx` も `kind` 列を書かず、replay の `apply_replan_step` は「`kind` は書き換えない」と同じ動きを写していた。
   このため行は `kind = task`、行の `spec` と計画の unit は leaf になった。
3. 人が v2 を承認すると、dispatcher の子 task の生成（`tree_units.rs`）が行の `kind == task` で change-map を選び、計画の unit
   （leaf）から子を組み立てる `task_ops::tree::build_child_task` が「unit change-map is not a task unit」を返し、unit は
   `child_create_failed` で failed になった（症状の出た段）。
4. 逆向き（leaf → task）では、行は leaf のまま WU の run として走り、子 task は作られなかった（黙って違う動きをする）。

## 変更

- `phase 1: replan で unit の kind を task と leaf の間で変えたときの行の kind を直し、始まった行の変更を採用の前に拒む`
  - task-core: `execution_plan::kind_bound_state`・`kind_change_errors`（純粋関数）と `PlanValidationError::StartedUnitKindChanged`。
    子 task のある kind task の行、WU ブランチ・commit・lease を持つか `running` の leaf の行の kind を task と leaf の間で変える計画を、
    planner に分かる理由（kind を元に戻すか、新しい key で書く）で返す。
  - task-core store: `update_work_unit_tx` が `kind` 列も書く。
  - task-ops: `plan_replan` は持ち越す行の `kind` を新しい版の unit の kind にし、`kind_change_errors` で始まった行の変更を拒む。
    replay の `apply_replan_step` も `kind` を書き換える（「`kind` は書き換えない」の記述を消した）。
  - task-dispatch: `prepare_adoption`（unit の gate の後、設計 review と root の承認の前）で `kind_change_errors` を呼び、planner の
    再試行に返す。採用の前の検証の失敗の文言を「計画が採用の前の検証に通りませんでした」に直した（以前の「子 Task の提案が委譲の
    検証に通りませんでした」は repos・kind の検証の失敗にも出ていた）。
  - 互換の分岐・切替 flag は置いていない。

## 試験（一時 DB と偽のアダプタだけ）

- `crates/task-dispatch/src/dispatcher/tests/tree_kind_change.rs`（新規）
  - `replan_turns_a_held_task_unit_into_a_leaf_that_runs`（task → leaf。本番 R4 の再現）: v1 の承認待ちを人が replan に回し、
    v2 で leaf にした unit m は行の kind が implement になり、WU の run で done。子 task は無く、`child_create_failed` も無い。
  - `replan_turns_a_held_leaf_into_a_task_unit_with_a_child`（leaf → task）: 行の kind が task になり、m から子 task が作られて done。
  - `replan_cannot_turn_a_task_unit_with_a_child_into_a_leaf`（子 task がある行）: 子が failed の unit c を親の replan で leaf に
    書き直した計画は採用されず（「unit c has already started (child task …)」が planner の再試行の理由に 1 件）、再試行の計画
    （kind task のまま）で 2 回目の子が done。計画は 2 版。
  - 3 本とも `assert_replay_is_clean`（replay の `work_units` の kind を含む突き合わせ）を通る。
- `crates/task-core/src/execution_plan/tests.rs` の `kind_change_errors_reject_only_started_rows_crossing_the_task_boundary`:
  leaf の行の状態（WU ブランチ・lease）での拒否、始まっていない行・leaf の kind どうし・done・superseded の行を拒まないこと。
- 試験が不具合を検出することの確認（提出前に一時的に修正を外して実行し、元に戻した）:
  - 持ち越しの `row.kind = wu_spec.kind` を外すと、task → leaf と leaf → task の 2 本が落ちる。
  - `kind_change_errors` の 2 か所の呼び出しを外すと、子 task がある行の 1 本が落ちる。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo nextest run -p task-dispatch tree_kind_change` → 3 passed
- `cargo nextest run -p task-core kind_change_errors` → 1 passed
- `bash scripts/dev/test-affected.sh` → 下の「提出時の検査」に記す
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 全体の試験（`cargo nextest run --workspace`）は走らせていない（ADR 2026-10-20-full-tests-at-release-only）。

## 本番への移行

- config・migration の変更は無い。release の後、新しい replan から行の kind が新しい版に合う。
- この修正より前の replan で kind を変えた未完の行が本番に残っていれば、採用後に同じ失敗（task → leaf）か、違う動き
  （leaf → task が WU の run で走る）をする。operator が読み取りだけで確かめる手順:
  `sqlite3 -readonly <本番 DB> "SELECT task_id, key, kind, status, json_extract(json, '$.kind') FROM work_units WHERE status NOT IN ('done','superseded','cancelled') AND kind != json_extract(json, '$.kind')"`
  （`json` 列は `WorkUnitSpec` の JSON で、`kind` は snake_case の文字列。0 行でなければ、その task を replan すると新しい release が
  行の kind を直す）。
- `celerisctl replay` で、この修正より前に kind を変えた replan を持つ task は、その行の `kind` が DB と食い違って出る
  （ADR D3。直す前の不具合の記録で、replay の例外にはしない）。R4 の change-map は v3 で kind task に戻したので食い違わない。

## 未解決

- 人の replan（`POST …/execution-plan/replan` → `plan_replan`）で子 task のある行の kind を変える経路は、task-core の単体試験と
  同じ関数で守られるが、API の結合試験は足していない。

## 提案

- 無し。
