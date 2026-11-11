---
title: plan の unit が別の task の完了・取り込みを待てるようにする（人の決定を待ちの代わりに使わない）
tasks: [01M74TM8Y43C5EMYF7FP525DZ5]
status: done
updated: 2026-11-11
---
# plan の unit が別の task の完了・取り込みを待てるようにする（人の決定を待ちの代わりに使わない）

完了日: 2026-11-11

## 結論
- /3 の計画の unit の `depends_on` に `task:<task id>` を書けるようにした。unit は依存先が done で、root 同士なら依存先の branch が
  既定 branch に入るまで始まらない。判定は task 全体の `depends_on` と同じ関数（`wait::dependency_done` と、M10 から切り出した
  `task_ops::dep_integration::dependency_integration`）を呼ぶ。判定のコードの写しは無い。
- 依存先が failed・cancelled・取り込みの破棄なら unit は始まらず、生存確認が既存の `StallDetected`・受信箱の `stalled` で 1 回知らせる。
  受信箱の新しい種類は作っていない。待ちの理由 M11 `task_dependency` は人の操作待ちではないので `human_wait_aging` にならない。
- planner の指示と設計 review の観点に「別の task を待つときは決定を置かず `task:<id>` を書く」を足した。
- 全体の試験（`cargo nextest run --workspace`、doc test の一括、`test-parallel.sh`）は走らせていない。本番の DB・process には触っていない。

## 変更（commit の件名）
- `phase 1: unit の task:<id> 依存（木の外の task の完了・取り込みを待つ）`
- `phase 1: health の悪化を直す（task_deps を既存の単位へ、解決を task-ops へ移す）`
- `phase 1: validate_for_task の doc を adopt_plan の doc から離す（clippy doc_lazy_continuation）`
- `phase 1: 検証が委譲の親・祖先と子を経由する循環を拒む（review の差し戻し）`
- `phase 1: unit_task_deps::resolve を resolve_pending_units に改名（公開面の名前の重複を増やさない）`
- `phase 1: 進捗に attempt 2 の commit を足す`
- `phase 1: 進捗に attempt 3 の状況を足す`

## review の差し戻しへの対応（attempt 2）
- 指摘: 検証が委譲の親（`tree` を持たない子の `parent_id`）を指す `task:` 依存を通し、依存先の子を経由する循環も通した。
  自分の木を `tree.root_id` だけで数え、循環の辺に「親は子を待つ」が無かったため。
- 直し方（`task_ops::unit_task_deps::plan_errors`。判断の純粋関数 `task_dependency_errors` は変えていない）:
  - 自分の木 = 自分・祖先（`parent_id` と木の親を辿る）・子孫（`children` を辿る）・自分と祖先が属する木の節点（`tree_tasks`）。
    委譲の兄弟は木の外のまま（待てる）。
  - 循環の辺に子（`children` と、有効な計画の kind task の unit の `child_task_id`）を足した。
  - ADR D4・`InOwnTree` の文・planner の指示・`docs/api/v1/api.md` の /3 の項を合わせて直した。
- 試験: `dispatcher::tests::unit_task_deps::plan_validation_rejects_delegation_ancestors_and_cycles_through_children`。
  委譲の親・祖父・孫は `InOwnTree`、委譲の子を通して owner を待つ root は `Cycle`、委譲の兄弟は通る。直す前の
  `plan_errors` に戻して走らせると「the delegating parent」で落ちる（left `[]`、right `[InOwnTree]`）ことを確かめた。

主な file:
- ADR: `agent-docs/adr/2026-11-11-unit-task-dependencies.md`（実装の前に書き、実装に合わせて D2・D4・D5 の置き場所を直した）。
- task-core: `execution_plan/task_deps.rs`（書き方・検証 D1・D4）、`wait/task_deps.rs`（解決 D2・D3・D5・D6）、`wait.rs`
  （`dependency_done`、`TaskWait::TaskDependency`、`plan_stall` が `task:` 依存を名指しの待ちとみなす）、`tree/liveness.rs`
  （`task_dependency` の事実と分類）、`execution_plan/validation.rs`（`validate_v3` は `task:` を段の規則に数えない）、
  `execution_plan/scheduling.rs`（行の無い依存の検査から外す）。
- task-ops: `unit_task_deps.rs`（事実の読み取り・`settle_external`・`plan_errors`）、`dep_integration.rs`（`dependency_integration`
  を切り出し）、`wait.rs`（M11）、`tree.rs`（生存確認の事実）、`inbox.rs`（`delivery_skipped` の文面に unit で待つ task も載せる）、
  `execution.rs`（人・CoS の採用と replan の検証 `validate_for_task`）。
- task-dispatch: `dispatcher/work_units.rs`（`resolve_child_dependencies` は task-ops を呼ぶだけ）、`dispatcher/planner_flow.rs`
  （`plan_preflight`）。
- task-worker: `claude_code/prompt.rs`（planner の /3 の指示）、`change_rules/review.rs`（設計 review の観点）。
- API・文書: `docs/api/v1/api.md`（改訂の記録と /3 の項）、`docs/api/v1/api-v1.schema.json`、`web/api/generated/*`、
  `web/features/tasks/wait-reason.tsx`（`task_dependency` の表示）、`docs/architecture-map.md`（行を足した）。

## health の悪化への対応
最初の版は `test-affected.sh` の repo health で落ちた（task-core の新しい module `task_deps` が既存の循環に加わった、
dispatcher module の合計・`work_units.rs`・`dispatch_one`・`validate_v3`・`PlanValidationError::fmt`・`classify_node`・
`on_planner_finished` の行が増えた）。次のように直した。
- 純粋関数を既存の単位（`execution_plan`・`wait`）の子 module に置き、循環の単位を増やさない。
- 検証の理由は `PlanValidationError` の variant にせず別の型 `TaskDependencyError` にした（`fmt`・`validate_v3` を伸ばさない）。
- `child:` と `task:` の依存の解決を `task_ops::unit_task_deps::settle_external` に移した（dispatcher module の行は減った）。
- M11 は dispatcher のメモリでなく `task_ops::wait::task_waits` で組む（gate の variant と `dispatch_one` の分岐を足さない）。
- planner の検証は harness の検査の呼び出しを `plan_preflight` に置き換えて足した（`on_planner_finished` の行は同じ）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh`: 結果は報告本文に書く。
- `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 結果は報告本文に書く。
- `sh scripts/dev/progress-index.sh --check`: 結果は報告本文に書く。
- 試験 (a)〜(e): `crates/task-dispatch/src/dispatcher/tests/unit_task_deps.rs` の 7 件と
  `crates/task-core/src/wait/task_deps/tests.rs` の 6 件。

## 未解決
- review の検査の時間: task-core を変えたので `test-affected.sh` は 14 crate・nextest 8231 件・web・test-ts-verify・repo health を
  走らせる。nextest だけで 640〜930 秒かかり（load average 80 前後、24 core）、review の検査の上限 1200 秒を超えやすい。
  attempt 2 の review はこの時間切れで終わり、criterion 3〜6（attempt 2 の直し）は評価されていない。attempt 3 はコードを変えず、
  検査を提出時の HEAD で再実行した。
- daemon の `until_idle`（`is_idle`）は、unit が `task:` 依存だけを待つ ready の task を idle とみなさない（M10 はメモリの待ちで
  idle とみなす）。本番の常駐 daemon には効かない。試験・e2e の `until_idle` で使うときは待ち続ける。
- 依存先の成果を自分の branch に入れる unit（既定 branch の取り込み）は今どおり計画が置く。自動では入れない。

## 提案
- R5（01M6TYVJT53598CPKMGSQGA7XM）の決定 01M736HS1RZD12PDQHQAJGC4WR のような「main に入ったら答えて」の決定は、この変更の
  release 後の replan で `task:<R2/R3/R4 の id>` の依存に置き換えられる。答え済みの決定と events は変えない。

## 本番への移行
- config の変更は無い。DB の migration も無い。release で入る。
