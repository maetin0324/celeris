---
tasks: [01M74VC34XB4RPAB61AV4226YS]
---
# ADR 2026-11-11: replan で unit の kind を変えたら行の kind も変える（始まった行は採用の前に拒む）

- 状態: 採用（実装済み）。[ADR-0079](0079-recursive-task-decomposition.md) D4 (4)・D9 の replan の行の持ち越しに付け足す。
- 関係: [ADR-0079](0079-recursive-task-decomposition.md) D4 (3)（unit の gate）・D4 (4)（kind task の unit の子）・D8（root の計画の承認）・
  付記「R7-3」D3（退役した key の検証）、[ADR 2026-10-20 replan の依存変更と WU の作り直し](2026-10-20-replan-dep-change-wu-rebase.md)、
  [ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)。
- 記録: [進捗](../progress/2026-11-11-replan-unit-kind-change.md)。

## 背景

- 本番の健全性 v2 R4（task 01M6TYVJT4S29ZMP6P0D4831JG）で、計画 v1 の unit change-map は kind task の行として作られた
  （v1 は承認待ちのまま人が replan に回した）。v2 は change-map を leaf に書き直した。人が v2 を承認した直後に
  change-map は `child_create_failed`（「unit change-map is not a task unit」）で failed になった。
- 原因は 2 か所ある。
  1. replan の行の持ち越し（`task_ops::execution::plan_replan`）は、既存の行を写して `spec`・`depends_on`・`phase`・`seq` を
     新しい版に書き換えるが、`kind` を書き換えない。
  2. store の行の更新（`update_work_unit_tx`）は `kind` 列を書かない。replay（`apply_replan_step`）も「`kind` は書き換えない」と
     この動きを写していた。
- その結果、行の `kind`（task）と計画の unit の kind（leaf）が食い違う。dispatcher の子 task の生成（`tree_units.rs`）は行の
  `kind == task` で unit を選び、計画の unit（`PlanUnitSpec::is_task`）で子を組み立てるので、採用と承認の後に失敗する。
  逆向き（leaf → task）では、行は leaf のまま WU の run として走り、子 task は作られない。

## 決定

### D1. 始まっていない行は、replan で新しい版の kind に合わせる

- `plan_replan` の持ち越しは `row.kind` を新しい版の unit の kind にする。WU ブランチの作り直し（ADR 2026-10-20 D1/D2）は
  新しい kind で判定する（task の行は WU ブランチを持たない）。
- store の `update_work_unit_tx` は `kind` 列を、書き込む spec の JSON の kind から導く（`kind = json_extract(<json>, '$.kind')`）。
  行の kind の正本は spec で、列はその写しになる（行を作るときは `WorkUnitRow::new` が spec の kind を写す）。他の書き手は
  spec を変えないので kind も変わらない。
- replay の `apply_replan_step` も同じく `kind` を新しい版の kind にする。

### D2. 始まった行の「task と leaf をまたぐ」kind の変更は、採用の前の検証で拒む

- 行が kind を変えると引き継げない状態を持つなら、その行の kind を task と leaf の間で変える計画を検証の理由で返す。
  判定は task-core の純粋関数 `execution_plan::kind_change_errors`（`PlanValidationError::StartedUnitKindChanged`）。
  - kind task の行: 子 task（`child_task_id`）がある。子が走っていても、終わっていても同じ。
  - leaf の行（task・integrate 以外）: WU ブランチ（`branch`）・commit（`head_commit`）・lease がある、または `running`。
  - done・退役した行は見ない（done の不変条件は `validate_with` が見る）。leaf の kind どうし（implement → test など）の変更は
    WU ブランチをそのまま使えるので拒まない（D1 で行の kind を合わせる）。
- 理由の文（`UnitKindChange` の Display）は「kind を元に戻すか、新しい key で書く（古い unit は superseded になる）」を planner に示す。
- 呼ぶ所は 2 つ。
  1. planner の計画の採用の準備（`prepare_adoption`、unit の gate の後）。設計 review（D3.5）と root の計画の承認（D8）の前に
     planner の再試行に返す。unit の gate が kind を変えた場合もここで見る。
  2. `plan_replan`（人の replan・人の計画・planner の採用が通る書き込みの直前）。退役した key・daemon の足した WU の key の検証と
     まとめた `execution_plan::replan_row_errors` で呼ぶ。
- 採用の後に `child_create_failed`（「is not a task unit」）で落ちる経路は残らない。D1 で行と計画の kind は一致し、一致させられない
  行は D2 で採用されない。

### D3. 互換の分岐を置かない

- 旧い行の kind を保つ分岐・切替 flag は置かない。この変更より前に kind を変えた replan を持つ task を replay すると、その行の
  `kind` は新しい版の kind になり、DB の行（旧い kind のまま）と食い違って出る。これは直す前の不具合の記録であり、replay の
  例外にはしない（進捗の「本番への移行」に、読み取りだけの確かめ方を書く）。

## 代わりに考えたこと

- 子 task の生成で行の kind と計画の kind が違えば計画に合わせる: 症状の出た所の特例で、leaf → task の向きと replay は直らない。
- kind の変更をすべて拒む: v1 が承認待ちのまま replan された R4 の場合（行は始まっていない）にも planner の書き直しが要り、
  今回の余分な planner run と承認がなくならない。
