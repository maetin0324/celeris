---
tasks: [01M74TM8Y43C5EMYF7FP525DZ5]
---
# ADR 2026-11-11: plan の unit は別の task の完了・取り込みを `task:<id>` の依存で待つ（人の決定を待ちに使わない）

- 状態: 採用（実装済み）。実装は task 01M74TM8Y43C5EMYF7FP525DZ5。
- 関係: [ADR-0079](0079-recursive-task-decomposition.md)（/3 の計画・unit・決定 D7。木の中の依存は unit の key で書く）、
  [ADR 2026-10-17 root 同士の依存は取り込みまで待つ](2026-10-17-root-dependency-waits-for-integration.md)（M10。
  この ADR は同じ判定を unit の依存に広げる）、[task の待ちの理由](2026-10-11-task-wait-reasons.md)（M11 を足す）、
  [ADR 2026-10-23 段の順と計画の詰まり](2026-10-23-stage-order-and-plan-stall.md)（D4 の名指しの待ち・D5 の受信箱）、
  [ADR 2026-10-27 案件の停滞](2026-10-27-project-stagnation-inbox.md)（`human_wait_aging`）、
  [破壊的変更を既定とする](2026-10-11-breaking-changes-by-default.md)。

## Context

/3 の計画の unit の `depends_on` に書けるのは同じ計画の unit の key（kind task の子を含む）だけだった。木の外の task
（兄弟の root や別の root）を待つ書き方が無いので、planner は人の決定を置いて待ちの代わりにしていた。健全性 v2 R5
（01M6TYVJT53598CPKMGSQGA7XM）の planner は 2026-11-10T17:02Z に「R2・R3・R4 が main に入ったら答えてください」という
決定を置いた。依存先が main に入ったかは DB と git で決まる事実なのに、人が見張るしかなく、その間 `human_wait_aging` が
約 15 時間出続けた。task 全体の `depends_on` には待ちの仕組み（`wait::db_waits` の D5 と M10
`TaskWait::DependsOnIntegration`）があるが、task の後ろの段だけを別の task に待たせる書き方が無かった。

## Decision

- **D1 書き方**: /3 の unit の `depends_on` に `task:<task id>`（`TASK_DEP_PREFIX`。id は 26 文字の ULID）を書ける。
  leaf と kind task の unit のどちらにも書ける。木の中の task（自分・祖先・子孫・同じ木の task）は今どおり unit の key で
  書く。/1・/2 の計画では未知の依存として拒む。`task:` の依存は段の規則（同じ段の依存は 1 件まで・後の段へは依存しない）に
  数えない（`validate_v3` は `task:` の依存を飛ばす）。採用時の「行の無い依存」の検査（`dependency_without_row_errors`）は
  `child:` と同じく見ない。形（`task:` の後が ULID か）は D4 の検証が見る。
- **D2 満たされる条件**: task 全体の `depends_on` と同じ判定を同じ関数で行う。
  1. 依存先の状態が done であること。判定は `task_core::wait::dependency_done`（`db_waits` の D5 と同じ関数）。
  2. 計画を持つ task と依存先がどちらも木の子でない（root 同士）なら、依存先の branch `celeris/<id>` が既定 branch の祖先で
     あること。判定は `task_ops::dep_integration::dependency_integration`（M10 の `integration_wait` が依存先ごとに呼ぶ関数を
     切り出したもの。git の祖先判定・delivery の見送り・人の保留/破棄の読み取りはこの 1 か所）。対話・support task の扱いも
     M10 と同じ。
  満たされたかの判断は task-core の純粋関数 `wait::resolve_task_dependencies` が事実（依存先の状態と取り込みの cause）から
  決める。事実を集めるのは `task_ops::unit_task_deps::resolve`。満たされた unit を `pending → ready`（event の理由
  `task_dependency_met`）に上げるのは `task_ops::unit_task_deps::settle_external`（`child:` の子 Task の依存の解決と同じ関数。
  dispatcher の `resolve_child_dependencies` はこれを呼ぶだけにした）。依存先の成果をこの task の branch に入れるのは今どおり
  計画の仕事（既定 branch を取り込む unit を置く）で、この依存は「既定 branch に入った」までを保証する。
- **D3 依存先の失敗・取り込みの見送り**: unit は `pending` のまま始めない（`blocked(dependency_failed)` にしない。replan は
  起こさない。人が決めることなので planner の LLM run を回さない）。人への知らせは既存の受信箱の項目だけを使い、
  新しい種類は作らない。
  - 依存先が `failed`・`cancelled`、または人が取り込みを破棄した（`discarded`）: 依存先がもう成果を出さないので、木の生存確認
    （ADR-0079 D10・ADR-0074 F5-fix8）がこの task を `task_dependency_ended`（理由なし）に分ける。`liveness_timeout_secs`
    続くと既存の `StallDetected`・障害通知・受信箱の `stalled` の項目が 1 件出る（同じ止まり方の間は繰り返さない）。人は
    依存先をやり直す（retry・reopen。done になって取り込まれると次の tick で unit が始まる）か、計画を作り直して依存を外すか、
    task を止める。
  - delivery が取り込みを見送った（`delivery_skipped`）: 依存先の既存の受信箱の項目 `delivery_skipped` の文面に、待っている
    task を足す（M10 D4 と同じ `waiting_dependents_note`。unit の依存で待つ task も載る）。
  - 人が保留にした（`held`）: 人が自分で決めた保留なので新しい知らせは出さない（M10 と同じ）。待ちの理由に cause が出る。
- **D4 検証**: 採用の前に、どの採用経路（planner の計画・人の `POST`/`PUT`・CoS の `PUT`）でも次を確かめ、違えば計画を拒む。
  判断は task-core の純粋関数 `execution_plan::task_dependency_errors`（理由は `TaskDependencyError`。`PlanValidationError` とは
  別の型で、planner・人には同じく 422 `validation`・不正な試行の理由の文として返す）、事実の読み取りは
  `task_ops::unit_task_deps::plan_errors`。planner の計画は dispatcher の `plan_preflight`（harness の検査と同じ所）、人・CoS の
  採用と replan は task-ops の `validate_for_task` が呼ぶ。
  - `task:` の後が task id であること（`Malformed`）。
  - 存在する task であること（`NotFound`）。
  - 自分の木の task でないこと（`InOwnTree`）。自分の木は、自分・祖先（`parent_id` と木の親 `tree.parent_unit` を辿る。
    委譲の親〈`tree` を持たない子の `parent_id`〉を含む）・子孫（`children` を辿る。委譲の子と木の子）・自分と祖先が属する
    木の節点（`tree_tasks`。採用で `parent_id` を書き換えない子を含む）。祖先は子孫の完了を待つので、祖先を指す依存は必ず
    循環する。木の中の順序は unit の key で書く。委譲の兄弟（同じ親の別の子）は木の外なので待てる。
  - 同じ案件であること。計画を持つ task が案件に属さなければ書けない（`OtherProject`）。
  - 循環しないこと: 依存先から「待つ」辺を辿って、自分の木の task に着かないこと（`Cycle`）。辺は task の `depends_on`、
    子（委譲の子と木の子。親は子の完了を待つ。dispatcher の `parent_waits_for_children` と計画の kind task の unit）、
    有効な計画の終端でない unit の `task:` 依存と kind task の unit の子。辺は task の状態で刈らない（終わった task も
    辿る。余分に拒んでも planner は別の書き方を選べるが、見逃した循環では unit が黙って待ち続けるため）。辿る task は
    512 件までで、その先は辿らない。
- **D5 待ちの理由**: M11 `TaskWait::TaskDependency { task_id, cause, detail, unit_keys }`。task 一覧・詳細・木の `waits` を組む
  `task_ops::wait::task_waits`（計画の待ち D7〜D13 と同じ所）が `unit_task_deps::resolve` で組む。依存先が複数なら task_id 順の
  最初の 1 件（M10 D3 と同じ）。M10 と違い dispatcher のメモリには持たない: dispatcher の gate を増やさず、running の task
  （他の unit が走っている）にも unit の待ちを出すため。git を読むのは、pending の unit が `task:` で待つ依存先が done で
  M10 の対象のときだけ。
  `cause` は `unfinished`（done でない）・`unmerged`・`delivery_skipped`・`held`・`discarded`（M10 と同じ）・`failed`・
  `cancelled`。unit ごとの `UnitWait.waiting_on` には `task:<id>` がそのまま出る。この待ちは人の操作待ちではない:
  受信箱の `HUMAN_WAITS` に入らないので `human_wait_aging` の対象にならず、待ちの理由があるので `task_unscheduled` にも
  ならない（停滞の判定も同じ `task_waits` を読む）。木の生存確認は D3 の 3 つ以外を名指しの待ち（`task_dependency`）に分ける。計画の詰まり（`plan_stall`）は、
  解けていない `task:` 依存を持つ pending の unit があれば出さない（`child:` と同じ）。
- **D6 始めた後は止めない**: 判定は unit が `pending` の間だけ行う。`ready`・`running`・`done` になった unit は、依存先が
  再 review などで状態を戻しても止めない。
- **D7 planner と設計 review の指示**: planner の /3 の指示に「別の task の完了・取り込みを待つときは決定を置かず
  `task:<id>` を書く。人の決定は人にしか決められないことだけに置く」を足す。設計 review の観点に「決定の問いが
  『〜が終わったら／main に入ったら答えてください』のように DB と git で決まる事実を人に見張らせていたら
  `dual_source_of_truth` の finding にする」を足す。repo-health の policy の file は変えない。
- **旧い書き方**: 決定を待ちの代わりに使う書き方を勧める指示は無い（今回の確認で見つからなかった）。互換の分岐・切替 flag は
  置かない。答え済みの既存の決定と events は変えない。

## ADR-0079・M10 との関係

- ADR-0079 の決定（D7）は「人にしか決められない選択」のためのもので、別の task の到着を待つためには使わない。木の中の依存は
  ADR-0079 のまま unit の key（kind task の子は代理の unit）で書き、`task:` は木の外だけに使う。
- M10 は task 全体の待ち（`ready` の task を dispatch しない）、この ADR は unit の待ち（その unit だけを上げない）。
  判定の関数は共有し、待ちの理由の形（task id・cause・detail）も揃える。

## 試験

temp DB と一時 git repo だけ（外部ネットワークなし）。時間は注入した時計。dispatcher の試験
`dispatcher::tests::unit_task_deps`（store・git を使う (a)〜(e)）と、task-core の `wait::task_deps::tests`（純粋関数）。
- (a) 依存先が done でない間と、root 同士で既定 branch に入っていない間は unit が始まらず、待ちの理由に依存先と cause が出る。
  入ると次の tick で始まる。
- (b) 同じ計画の、task 依存を持たない unit は待たずに進む。
- (c) 依存先が failed・cancelled なら unit は始まらず、受信箱の `stalled` の項目が 1 件出る。
- (d) 検証が、存在しない task・自分の木の task・循環・別の案件を拒む。委譲の木（`tree` を持たない子）でも、委譲の親・祖先・
  子孫を指す依存と、依存先の子を経由する循環を拒み、委譲の兄弟は通す
  （`plan_validation_rejects_delegation_ancestors_and_cycles_through_children`）。
- (e) task 依存の待ちでは `human_wait_aging` が出ない。
