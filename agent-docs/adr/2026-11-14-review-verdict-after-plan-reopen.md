---
tasks: [01M7DYMCVS1PMDK2N0WW3ME9T8]
status: accepted
---
# ADR 2026-11-14: 最終 review の間に計画が開き直されたら、旧い判定を捨てて Task を ready に戻す

- 状態: 採用（実装は task 01M7DYMCVS1PMDK2N0WW3ME9T8 の WorkUnit late-review）
- 関係: [ADR 2026-11-11 Dispatcher の状態の owner と、判断と副作用の分け方](2026-11-11-dispatcher-state-ownership.md) の
  D7（今の実装で成り立たない反例は owner で直す）を、cex-late の「旧い review の合格」の反例について具体にする。前例は
  [ADR 2026-11-14 dispatch gate の昇格](2026-11-14-dependency-promotion-every-plan-version.md)。
  [ADR-0118](0118-review-target-sync-and-merge-candidate.md) D4 付記（検査中に snapshot が変わった判定は適用しない）と同じ場所で、
  同じく判定を適用しない。ADR 2026-11-11 と ADR-0118 の本文は変えない。

## 背景

v2 の計画の Task が最終 review（Reviewing）にいる間に、人の replan（`PUT /tasks/{id}/execution-plan`）、CoS の計画の差し替え、
planner の replan（段の repair の unit を足す）が `task_ops::execution::replan` で確定しうる。`replan` は Task の状態を見ずに
統合済みの段を開き直し（統合 WU を pending、新しい unit を ready）、Task を Reviewing のまま残す。

その後に旧い reviewer run（変更の前の候補を検査していた）の合格が届くと、`crates/task-dispatch/src/dispatcher/review_verdict.rs`
の `on_review_finished` は判定をそのまま適用し、Task を `review_pass` で Done にしていた。破棄するのは Task が Reviewing でない
ときと、検査中に target・HEAD・branch が変わったとき（`target_stale`）だけで、計画の変更は判定に入っていなかった。Done の時点で
有効な計画には未完の unit（新しい unit が ready、統合 WU が pending）が残る。CLAUDE.md の「完了はレビュアーか決定的な検査が
決める」に対し、review が見ていない計画で完了になる（観測は
[cex-late2.md の late-review 節](../progress/2026-11-11-health-v2-r4/cex-late2.md)、試験は
`cex_late_review_pass_after_replan_is_dropped`・`cex_late_review_pass_after_repair_is_dropped`）。

## 決定

### D1. 破棄の条件は SQLite の有効な計画の unit から導く

`on_review_finished` は、Task が Reviewing のとき、`ReviewerPassed`・`RequiredChecksEvaluated`・判定の適用（合格・不合格・repair・
health の停止のどれも）より前に、`store.work_units_for(task)` を既存の述語 `task_core::next_work_unit` で見る。走らせる unit
（ready か needs_continuation。`NextStep::RunWorkUnit`）が残れば、判定は旧い計画の候補に対するものなので、合格も不合格も捨てる。

- 計画の無い Task（unit が 0 個、`Stuck`）と、unit がすべて done の Task（`AllDone`）では挙動は変わらない。
- blocked・failed だけが残る計画（`Stuck`）も捨てない。ADR-0120 の integration repair の打ち切り（fallback）では、repair の WU が
  `blocked(plan_issue)` のまま Task が同期していない HEAD で最終 review に進む。これは review の間の計画の変更ではない
  （`dispatcher::tests::target_sync::integration_repair_fallback_*` が守る）。
- replan・段の repair は、新しい unit（依存の満たされたもの）を ready にし、統合済みの段の統合 WU を pending に戻す。走らせる unit が
  残るのは、最終 review の間に計画が変わったときである。依存の満たされない pending だけを足す replan（`child:`・`task:` の外部の
  依存だけを持つ unit）はこの条件に当たらない（範囲外。起きたら別に扱う）。
- 正本は SQLite の unit の行だけにする。review の subject・メモリの `ReviewEntry` に計画の版を持たせない（新しい状態を足さない）。

### D2. 破棄の記録は既存の破棄と同じ処理で行う

判定を適用しない場合は 3 つになる: Task が Reviewing でない・D1 の計画の開き直し・検査中の snapshot の変化（`target_stale`）。
`on_review_finished` はこの 3 つを 1 つの分岐で閉じる: `WorkerFinished{role: reviewer}` の `end` と run index を `Cancelled` に
する。前の 2 つでは `ReviewerPassed`・`ReviewVerdict`・`RequiredChecksEvaluated` を書かない。旧い合格は新しい候補に束縛されず、
`ReviewerPassReused` の材料にもならない。`target_stale` の場合の扱い（`ReviewerPassed` を先に残し、reviewing のまま次の tick で
再同期する）は変えない。

### D3. 新しい遷移 `Trigger::ReviewPlanReopened`（Reviewing → Ready、attempts 不変）

D1 で捨てたとき、`WorkerFinished` を書いた後に、Task を `Trigger::ReviewPlanReopened`（`reason = "review_plan_reopened"`）で
Reviewing → Ready に戻す。Ready に戻った Task は、dispatch gate が新しい版の unit（新しい unit、開き直された統合 WU）を走らせ、
終わったら新しい候補で最終 review をやり直す。

- attempts を消費しない。`retry_policy::attempt_history` の試行の表にも入れない（review の不合格ではない）。
- `plan_awaits_final_review` の「判定」の表（`review_pass`・`review_fail`・`review_repair`）にも入れない。判定は適用していない
  ので、新しい版はまだ最終 review を受けていない。
- `WorkerFinished` の追記と遷移は別の書き込みにする（既存の破棄の分岐の書き込みをそのまま使う）。間で daemon が止まると Task は
  Reviewing のまま残るが、再起動の後の review の結果も D1 の条件で捨てられ、そのときに遷移する（reviewer run が 1 回余計に走る）。
- 既存の trigger は意味が違うので使わない。`Interrupt` は人のコメントの割り込み（`reason = "comment"`、ADR-0044 D2）。
  `ReviewRepair` は review の不合格から repair の unit を作る遷移で、`plan_awaits_final_review` では判定として数える。
  `ReviewFail` は attempts を消費する。

### D4. 選ばなかった案

- review の subject に有効な計画の id を束縛し、判定の前に `execution_plan_active` と比べる案。subject は再起動の後に
  `runs/<run_id>/result.json` から組み直す（`subject_from_run_dir`）ので、計画の id を run の dir に書き足す必要があり、計画の状態の
  正本が 2 つになる。D1 は SQLite の unit の行だけで決まる。
- `task_ops::execution::plan_replan` が Reviewing の Task の段を開き直したとき、同じ transaction で Task を Running に戻す案。
  replan の書き込みは task-ops と store にあり、呼び元が 3 つ（API・CoS・planner）ある。旧い判定を適用しない責務は判定を適用する
  `on_review_finished` にあり、ADR-0118 D4 付記の stale の扱いと同じ場所に置く方が owner が 1 つで済む。

### D5. 結果

- 計画を変えてから旧い reviewer run が終わるまで、Task は Reviewing のままで、新しい unit は走らない。reviewer run は最後まで走り、
  その判定は捨てられる（reviewer の費用は戻らない）。走っている reviewer を計画の変更の時点で止めることは、この ADR の範囲外とする。
- D7 の規則のとおり、反例の試験を足した unit ではなく、計画を作り直した後の unit がこの修正をした。ADR 2026-11-11 の D7 付記の更新は
  親 task の段 close-d7 が行う。

## 試験

- `crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs` の `cex_late_review_pass_after_replan_is_dropped`
  （人の replan で WU `c`）と `cex_late_review_pass_after_repair_is_dropped`（planner の replan で段の repair）: reviewer run を
  fake adapter の gate で止め、計画の変更を確定してから合格を届ける。旧い合格は Task を Done にせず、新しい unit の done の後の
  review だけが、新しい unit を含む候補で Done にする。修正の前は 2 本とも `status=Done` で落ちた。
- `crates/task-core/src/transition/tests.rs`: `ReviewPlanReopened` は全 kind で Reviewing → Ready・attempts 不変、他の状態から
  は無効。`crates/task-core/src/retry_policy/tests.rs`: `review_plan_reopened` の遷移は試行に数えない。
