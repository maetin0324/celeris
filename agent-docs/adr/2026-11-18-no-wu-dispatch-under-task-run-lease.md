---
tasks: [01M7Q70NEJ7NABAYA4C0F9CS6Z]
status: accepted
---
# ADR 2026-11-18: task 単位の run が task の lease を持つ間は、その task の WU を起こさない

- 状態: 採用（実装は task 01M7Q70NEJ7NABAYA4C0F9CS6Z の WorkUnit fix-test）
- 関係: ADR-0074 D1.3 3.・D1.5（並列 WU の 2 本目以降と工程の lease）、ADR-0079 付記「R6-1」D1（PlanGate を通っていない版の
  unit は起こさない）、ADR 2026-11-07-health-review-integration D3.5 5.（設計 review の不合格が続いたら計画を採用して人に
  回す）、ADR 2026-11-14-dispatch-admission-order（2 本目以降が飛ばす gate）、ADR 2026-10-03-ownerless-running-runs
  （lease の持ち主は 1 つ）。どの ADR の本文も変えない。

## 背景（事象）

本番の task 01M7MZTKB29VF5TX7FCQ15XEMW の events（seq 1088〜1737）で、次のことが起きた。

1. 03:31:39Z（seq 1087〜1092）: 計画の設計 review が 2 回続けて不合格になり、dispatcher は計画 v2 を採用したうえで人の承認に
   回した（`execution_planned` version 2 → `transitioned ready→blocked awaiting_plan_approval` → `plan_approval_requested`
   reasons `design_review_failed:2`）。この採用で、v1 で `blocked(plan_issue)` だった WU risk-rule が `ready` になった
   （seq 1088、reason `replan v2`）。
2. 03:34:32Z（seq 1093〜1094）: 人が承認に replan で答え、task は `blocked→ready`（`plan_replan`）。
3. 03:34:38Z（seq 1096〜1097）: planner の run 01M7PBH1MXT9X86S09NB6W49WB が dispatch され、task は `running` になった。
   task の lease の保持者はこの planner の run id。
4. 03:34:41Z〜03:41:56Z（seq 1102〜1664）: planner が走る間に、risk-rule が 56 回 dispatch され、うち 55 回は約 3 秒後に
   `interrupted: aborted (task no longer running under this lease)`（usage null）で `needs_continuation` に戻った。
   56 回目は planner の終わり（seq 1675、03:42:06Z `running→ready planned`）の後の seq 1736 で同じく打ち切られた。

同じ形は 2026-11-10 の task 01M6TYVJT0Z1CBGNK8VTNK8ZJH でも起きていた（設計 review の不合格 → 承認待ち → 人の replan →
planner の run の間に WU launcher-login-fix の dispatch と打ち切りが 30 回）。2026-11-11 07:58Z の task
01M6TYVJT4S29ZMP6P0D4831JG も承認待ちに replan で答えたが、承認待ちの版の unit が `ready` でなかった（adr は
`blocked(decision)`、ほかは `pending`）ので起きなかった。

## 原因

候補 (a)「task の lease を planner など task 単位の run が持つ間も、その task の WU を dispatch する」が原因である。
候補 (b)「採用されていない計画の版で WU を ready に変える」は、この事象が起きる前提（起こせる unit がある）を作ったが、
設計どおりの挙動で不具合ではない。

### (a) 並列 WU の 2 本目以降の選び方が、task の lease の保持者を見ていない

- `crates/task-dispatch/src/dispatcher/work_units.rs` の `dispatch_parallel_work_units`（1831 行〜）は、doc comment で
  「このインスタンスが既に run を持っている（＝工程の lease の持ち主の）Task だけを対象にする」と定めている。実装
  （1840〜1846 行）は `running_tasks_with_runnable_work_units` が返す task のうち `self.supervisor.runs_for_task(task.id) > 0`
  のものを対象にする。「run を持っている」と「工程の lease の持ち主である」が同じだという前提は、task 単位の run
  （planner）が task の lease を持つときに成り立たない。planner の run も `runs_for_task` に数えられる
  （`crates/task-dispatch/src/run_supervisor.rs` 165 行）。
- `crates/task-core/src/store/execution/work_units.rs` の `running_tasks_with_runnable_work_units`（466〜487 行）は、
  `status = running` で、phase のある `ready`/`needs_continuation` の WU を持つ task を返す。lease の保持者は見ない。
- `crates/task-dispatch/src/dispatcher/dispatch_run.rs` の `dispatch_one` は、2 本目以降（`second_pass`、45 行）では
  task の lease を取らず（602 行 `let task_lease = second_pass || { … }`）、すぐに WU の lease を取る。
  `acquire_work_unit_lease`（`crates/task-core/src/store/execution/work_units.rs` 350 行〜）が見るのは task が
  `running` であること（364 行）と WU が `ready`/`needs_continuation` であることだけで、task の lease の保持者は見ない。
  `dispatch_admission::admit` も、2 本目以降では「同じ task の run」の gate を飛ばす
  （`crates/task-dispatch/src/dispatch_admission.rs` 541 行 `if first && …`）。
- その結果 WU の run が起動し、次の tick の `abort_stale_runs`（`crates/task-dispatch/src/dispatcher/leases.rs` 512〜541 行。
  tick の中の呼び出しは `crates/task-dispatch/src/dispatcher.rs` 1727 行）が `run_holds_lease`（`leases.rs` 738〜755 行）で
  打ち切る。task の lease の `worker_run_id` は planner の run id で、WU の run id と違い、`phase:` で始まらない
  （`is_phase_lease_holder`、`dispatcher.rs` 359 行）ので `false` になる。`run_holds_lease` の判定は正しい
  （lease の持ち主は 1 つ）。
- 打ち切った task は `mark_just_aborted` でその tick だけ dispatch から外れる（`dispatch_parallel_work_units` 1844 行の
  `was_just_aborted`）。次の tick では WU が `needs_continuation`（起こせる状態）に戻っているので、また dispatch される。
  planner が終わって task が `running` でなくなるまで、これが続く。

2 本目以降の経路は、`wu_dispatch_gate`（`work_units.rs` 412 行〜）の判定も通らない。`ready` の task の 1 本目なら、
人の replan の後は `RunPlanner`（427〜440 行）を返し、承認待ちの版の unit は `human_gate_hold`（445〜466 行。
`task_ops::plan_gate::plan_gate_state` が `Pending` を返す。`crates/task-ops/src/plan_gate.rs` 484〜487 行・511 行）で止める。
2 本目以降は「1 本目がこれらの gate を通って工程の lease を取った」ことを前提にしているが、その前提を確かめていない。

### (b) 承認前の版で WU を ready にするのは設計どおり

- 設計 review が 2 回不合格になると、`crates/task-dispatch/src/health_review/design_verdict.rs` の 131〜139 行が
  `adopt_reviewed_plan`（154 行〜）で計画を採用し、`design_review_failed:<n>` の理由で人の承認に回す（ADR
  2026-11-07-health-review-integration D3.5 5.「その計画を採用したうえで PlanGate で人に回す」）。
- 採用は `task_ops::execution::plan_replan`（`crates/task-ops/src/execution.rs` 568 行〜）で行う。未完了で新しい版にも残る
  key は、依存がすべて done なら `ready`（725〜737 行・811 行）、段のある計画では段の障壁つきで `ready` を決め直す
  （1003〜1036 行）。seq 1088 の `blocked→ready`（reason `replan v2`）はここで積まれた。
- 承認されていない版の unit を起こさないことは、dispatch の時点で `human_gate_hold` が守る（ADR-0079 付記「R6-1」D1）。
  採用の時点で unit の状態を止める仕組みではない。既存の試験
  `plan_gate_replan_does_not_dispatch_the_unapproved_version`（`crates/task-dispatch/src/dispatcher/tests/human_gates.rs`
  121 行〜）もこの設計を確かめている。

(b) を変える（承認まで unit を `ready` にしない）と、計画の採用・承認・replan の既存の挙動と試験の期待値が変わる。
(a) を直せば、planner が走る間に起こされる unit は無くなり、planner の後は新しく採用された版で unit の状態が決め直される。
よって (b) は変えない。

## 決定

### D1. 並列 WU の 2 本目以降は、task の lease の保持者が工程（`phase:`）のときだけ起こす

`dispatch_parallel_work_units` は、対象の task の `lease` が有り、その `worker_run_id` が `is_phase_lease_holder` を満たす
ときだけ、2 本目以降の WU を選ぶ。doc comment の「＝工程の lease の持ち主の」を、`runs_for_task > 0` の近似ではなく
lease の保持者で確かめる。`runs_for_task > 0`（このインスタンスが run を持っている = 引き継ぎ中の別インスタンスの task に
手を出さない）と `is_integrating`・`was_just_aborted`・`halted_by_pause` の判定は今のまま残す。

lease の保持者の書式（`phase:` の接頭辞）の判定は、`run_holds_lease` と同じ `is_phase_lease_holder` を使う
（判定を 2 か所に書かない）。

### D2. 採用・承認・replan の unit の状態の決め方は変えない

`plan_replan` の `ready`/`pending` の決め方、設計 review の不合格の後の採用と PlanGate、`human_gate_hold` は変えない。
人の承認・replan・取り下げと、工程の lease の下での WU の通常の dispatch（1 本目・2 本目以降）の挙動と既存の試験の
期待値は変えない。

### D3. 反例試験

planner の run を試験が開けるまで止める adapter（`tokio::sync::Semaphore` で run を止める。
`crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` の `GatedAdapter` と同じ形）を使い、出来事待ちで再現する。
sleep・CPU の負荷・壁時計の経過に頼らない（tick は試験が `d.tick()` で進める）。

1. 段のある計画（/3）を採用して人の承認待ち（`PlanGateState::Pending`）にする。版には、依存の無い leaf の unit（phase が
   有り、kind が `implement`）を入れ、`ready` にしておく。
2. `task_ops::plan_gate::plan_gate(…, PlanGateAction::Replan, …)` で答える。
3. tick を進め、planner の `WorkerStarted{role: Planner}` が events に出るまで待つ。planner の run は adapter が止めている。
4. さらに数 tick 進め、その間に leaf の unit の `WorkUnitTransitioned{to: running}` と、
   `aborted (task no longer running under this lease)` の `WorkerFinished` が無いこと、unit の `runs` が 0 のままであること、
   task の lease の保持者が planner の run id のままであることを確かめる。
5. planner を開け、次の版を承認した後は、unit が通常どおり dispatch されて task が進むことを確かめる。

D1 の判定を外すと 4. の最初の tick で unit が `running` になり、次の tick で打ち切られるので、試験は落ちる（失敗注入で
確かめ、進捗に書く）。

既存の `plan_gate_replan_does_not_dispatch_the_unapproved_version` がこの不具合を検出できない理由: (1) planner の adapter
がすぐ返すので、planner の run が走っている間に別の tick の `dispatch_parallel_work_units` が回らない。(2) 承認待ちの v1 の
unit は kind `task` で、`running_tasks_with_runnable_work_units` は kind `task` の unit を対象にしない。

## 検討した案

- `acquire_work_unit_lease`（store）で task の lease の保持者が `phase:` であることを確かめる: lease の保持者の書式
  （`PHASE_LEASE_PREFIX`）は task-dispatch の概念で、store に写すと同じ規則が 2 か所になる。また store で断ると、
  route の選択などを済ませた後で毎 tick 断ることになる。採らない。
- `dispatch_admission::admit` に「task の lease が工程か」の fact を足す: admit は gate の順序を持つ純粋関数で、2 本目以降を
  選ぶのは `dispatch_parallel_work_units` の責務。選ぶ側で直せば admit に分岐を足さずに済む。採らない。
- (b) を変え、承認まで unit を `ready` にしない: D2 のとおり、既存の承認・replan の挙動と試験の期待値が変わる。採らない。
- `abort_stale_runs` で planner の lease の下の WU の run を打ち切らない: lease の持ち主が 2 つになる
  （ADR 2026-10-03-ownerless-running-runs）。採らない。

## 不変条件

- task の lease の保持者が task 単位の run（planner など）の間は、その task の WU の run を起こさない
  （D3 の試験で確かめる）。
- 工程の lease の下の WU の 2 本目以降の dispatch は今のまま（`work_units.rs` の試験
  `three_independent_units_run_in_parallel_and_integrate`・`two_parallel_units_at_a_time_when_the_limit_is_two`・
  `ready_task_first_run_beats_a_second_parallel_unit` など）。
- 承認されていない版の unit は起こさない（`human_gates.rs` の既存の試験）。
- `run_holds_lease` の判定（lease の持ち主は 1 つ）は変えない。
