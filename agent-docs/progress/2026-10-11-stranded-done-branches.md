---
title: done task の取り残し調査
updated: 2026-10-11
status: running
tasks: [01M4M8D4ANYGCWVZ0A0WSW53X7]
---

# done task の取り残し調査

## 原因

第一の根因は、delivery の開始が task の done 遷移や「review 合格」全体に結び付いていないこと。`crates/task-dispatch/src/dispatcher/review_spawn.rs:620-630` は `reviewer_run` が `Some` の場合にだけ `task_ops::delivery::begin` を呼ぶ。`reviewer_run` は `needs_reviewer_run(&task)` が真、つまり既存の acceptance に `Check::Reviewer` があり、さらに reviewer を選べた場合にだけ作られる（`crates/task-dispatch/src/review.rs:169-174`、`review_spawn.rs:345-354`）。Reviewer 条件のない task の review は他の check で合格して done になれても `begin` を通らない。delivery 自身が追加する部署内 reviewer criterion は `begin` の後（`crates/task-ops/src/delivery.rs:375-376`）なので、これを開始条件にできない。

計画付き task の最終レビューにも独立した抜け道がある。全 WU が終わった計画は `dispatch_run.rs:83-86` から `start_final_review_from_ready` に入り、`review_spawn.rs:129-171` は `Trigger::PlanComplete` で reviewing に遷移させて `spawn_review` する。この経路も reviewer run が既存の `Check::Reviewer` に基づいて選ばれなければ `begin` を呼ばない。WU の統合段（`StartIntegration`）は親 task の branch に成果をまとめる段で、root task の main delivery 開始ではない（`dispatch_run.rs:78-82`）。

二つ目の silent skip は delivery policy の対象外。`prepare` は project_id がない場合、`policy.projects` に project が無い場合、support task、木の子である場合に `Skip::Silent` を返し、`begin` は何も記録せず `None` を返す（`crates/task-ops/src/delivery.rs:172-181,363-365`）。設定は `config/dispatch.rs:163-169` で `[selfdeploy].delivery_projects` から渡され、既定値は空（`crates/celeris/src/config/selfdeploy.rs:56-58,84-95`）。config は delivery 対象 project を agent-platform の project ID（`01M2WTS3DKNZBSZ2JMVB4CZMBW`）だけに限定していることを read-only で確認した（`~/.config/celeris/config.toml:370-372`）。従って同 ID の project root なら config 不一致は今回の根因ではない。別 project ID なら silent skip する。

与えられた事実では 01M4D7RVKX と 01M4F3V643 の task row、acceptance、task の全 event payload をこの run から取得できず、個別の `Check::Reviewer` 有無や project_id は直接確認できていない。events に `phase_integrated {merged:[], head:""}` 以外の delivery event が無いという依頼文の事実と上記の制御フローからは、delivery begin が一度も呼ばれなかった、または silent skip したと推論できる。project config が該当 ID を含むため、最も整合する原因は reviewer run を伴わない review 経路。これはコード上、done 遷移の順序による後段の取り逃しではなく、begin の呼び出し条件が reviewer run に狭く結合している問題。

`crates/celeris/src/delivery.rs:158-176` の `tick` は既存 deliveries 行だけを走査する。行が無い task の branch を検索せず、未記録 task を検出できない。木の子を skip する安全策もある。よって begin 未到達や silent skip 後に補完する daemon 経路は現状ない。

## 経路表

| task 経路 | review / done 経路 | begin 到達条件 | 判定 |
| --- | --- | --- | --- |
| Atomic root | implementation 後 `spawn_review`。reviewer run の選択は acceptance の `Check::Reviewer` がある場合のみ | reviewer run を作成できた場合だけ `review_spawn.rs:345-354,620-630` | reviewer 条件なしで pass/done になると未到達。今回の2件はこの経路が最有力（個別 row 未確認） |
| Compound root | WU done → `StartIntegration` → 全計画終了後 `FinalReview` → `PlanComplete` → `spawn_review` (`dispatch_run.rs:78-86`; `review_spawn.rs:129-171`) | final review で reviewer run が選ばれた場合だけ | `Check::Reviewer` なしなら未到達。integration は main delivery の代替でない |
| 木の子 | child review 前に child branch を commit (`review_spawn.rs:368-373`)、親工程の統合で親 branch に取り込む | reviewer run があれば `begin` 呼び出し自体には到達し得る | `prepare` が `is_tree_child` を silent skip (`delivery.rs:175-181`)。main delivery 対象外は意図された設計 |
| project / kind による除外 | 上記各 review 経路の後 | `begin` まで到達しても `prepare` 判定 | project_id 無し、config 対象外、support は silent skip (`delivery.rs:172-181`) |

## 再現試験の形

`task-dispatch` の dispatcher fixture と `task-ops::delivery` の store fixture を使い、git repo/worktree fixture に既定ブランチと task branch を用意する。project row、repo row、worktree marker、delivery policy の project ID を揃え、task を ready から実際に review pass → done に遷移させる。

1. Atomic root: `Check::Reviewer` を acceptance に持たない task を dispatch/review 完了させる。done を確認し、`store.delivery_get(task_id) == None`、deliveries 一覧に行が無く、delivery event も無いことを確認する。これは begin 未到達を示す。
2. Compound root: active plan と done の implementation WU、および必要な integrate WU を fixture に入れ、`Trigger::PlanComplete` の最終 review を通す。同じく Reviewer 条件なしで done、delivery 行なしを確認する。
3. Reviewer 制御: 同 fixture で acceptance に `Check::Reviewer` を追加し reviewer provider を用意する。`begin` に進み delivery 行または `delivery_skipped` が記録されることを確認する。次に policy project を外した fixtureでは、begin は呼ばれるが行・skip event とも無い silent skip を確認する。
4. 木の子: task に親を設定し reviewer 条件を付けて begin まで進め、delivery 行が無いことを確認。親工程 integration は別途親 branch に成果を入れることを確認し、child 自身を main delivery しない既定を固定する。

試験の決定的な主張は、本番 DB や daemon tick に依存させず、fixture の task status・acceptance・project policy・git refs を固定して `done` と deliveries 行の有無を検査すること。現在は調査 WorkUnit のため、試験追加・コード変更は行っていない。
