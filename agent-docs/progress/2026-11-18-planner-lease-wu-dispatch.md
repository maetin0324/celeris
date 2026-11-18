---
title: planner の run が task の lease を持つ間の WU の dispatch と約 3 秒の中断の繰り返し
tasks: [01M7Q70NEJ7NABAYA4C0F9CS6Z]
status: done
updated: 2026-11-18
---
# planner の run が task の lease を持つ間の WU の dispatch と約 3 秒の中断の繰り返し

完了日: 2026-11-18（原因の特定と ADR は WorkUnit root-cause、修正・反例試験・失敗注入は WorkUnit fix-and-test）

## 結論

- 原因は候補 (a)。task の lease を planner の run が持つ間も、並列 WU の 2 本目以降の経路
  （`dispatch_parallel_work_units`）がその task の `ready`/`needs_continuation` の WU を dispatch する。起動した WU の run は
  次の tick の `abort_stale_runs` に打ち切られ、WU は `needs_continuation` に戻り、また dispatch される。
- 候補 (b)（承認前の版で WU を `ready` にする）は、起こせる unit を作った前提だが、設計どおりの挙動で不具合ではない
  （ADR 2026-11-07-health-review-integration D3.5 5. と ADR-0079 付記「R6-1」D1）。変えない。
- 直す owner は `crates/task-dispatch/src/dispatcher/work_units.rs` の `dispatch_parallel_work_units`。task の lease の保持者が
  工程（`phase:`）のときだけ 2 本目以降を起こす。ADR: `agent-docs/adr/2026-11-18-no-wu-dispatch-under-task-run-lease.md`。

## 事象（本番 DB を読み取り専用で読んだ。task 01M7MZTKB29VF5TX7FCQ15XEMW）

読んだ方法: `sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"` で `events` を seq と `json_extract` で引いた
（コマンドは「証拠」節）。

| seq | 時刻（Z） | event | 意味 |
|---|---|---|---|
| 820 | 03:11:30 | `work_unit_transitioned` risk-rule running→blocked `plan_issue` | v1 の risk-rule が計画の不備を申告 |
| 821, 828 | 03:11:30 | `transitioned` running→ready `replan`、ready→running `dispatch` | planner の replan |
| 936, 1087 | 03:21:26, 03:31:39 | `design_reviewed` pass false（2 回） | 設計 review が 2 回続けて不合格 |
| 1088 | 03:31:39.625 | `work_unit_transitioned` risk-rule blocked→ready `replan v2` | v2 の採用で risk-rule が ready（(b)） |
| 1090 | 03:31:39.625 | `execution_planned` version 2 | v2 を採用（active） |
| 1091–1092 | 03:31:39.627 | `transitioned` ready→blocked `awaiting_plan_approval`、`plan_approval_requested` `design_review_failed:2` | 人の承認に回す |
| 1093–1094 | 03:34:32 | `transitioned` blocked→ready `plan_replan`、`plan_gate_decided` replan | 人が replan で答える |
| 1096–1097 | 03:34:38 | `transitioned` ready→running `dispatch`、`worker_started` role planner run 01M7PBH1MXT9X86S09NB6W49WB | planner が task の lease を取る |
| 1102 | 03:34:41.48 | risk-rule ready→running `dispatch` run 01M7PBH4HWDFMXSEMC5AKVKJVK | planner の run の間に WU を dispatch（(a)） |
| 1109–1110 | 03:34:44.47 | `worker_finished` `interrupted: aborted (task no longer running under this lease)` usage null、risk-rule running→needs_continuation `aborted` | 約 3 秒で打ち切り |
| 1111 | 03:34:48.38 | risk-rule needs_continuation→running `dispatch` | 約 4 秒後に再 dispatch |
| 1102–1664 | 03:34:41–03:41:56 | risk-rule の `dispatch` 56 回、打ち切り 55 回（seq 1109–1662、すべて usage null） | 繰り返し |
| 1675–1676 | 03:42:06.71 | `transitioned` running→ready `planned`、planner の `worker_finished` done | planner が終わる |
| 1736–1737 | 03:42:06.83 | 56 回目の run の打ち切り、risk-rule running→needs_continuation | 最後の 1 回 |
| 1867–1871 | 03:54:15–03:54:17 | risk-rule needs_continuation→ready `replan v3`、`execution_planned` v3、task と risk-rule の `dispatch` | v3 の採用後は通常どおり |

この task の `aborted (task no longer running under this lease)` は 57 件。55 件が seq 1109–1662、1 件が seq 1736、
残りの 1 件（seq 942、03:21:32Z）は設計 review の判定（seq 936）の後に起動した reviewer の run の打ち切りで、この事象とは
別の経路（「未解決」に記す）。

同じ形の過去の事象: task 01M6TYVJT0Z1CBGNK8VTNK8ZJH（2026-11-10）。設計 review の不合格 → 承認待ち（seq 982）→ 人の replan
（seq 984–985）→ planner の run（seq 987–988）の間に WU launcher-login-fix の dispatch と打ち切りが続いた（30 件、
seq 993 から）。2026-11-01 以降の承認の replan は 3 件で、残りの task 01M6TYVJT4S29ZMP6P0D4831JG（2026-11-11 07:58Z）は、
承認待ちの版の unit が `ready` でなかった（adr が `blocked(decision)`、ほかは `pending`）ので起きなかった。2026-11-01 以降の
同じ打ち切りはこの 2 task だけ。

## 原因（コードの file:行。基準は `$CELERIS_WU_BASE`）

### (a) 並列 WU の 2 本目以降が、task の lease の保持者を見ずに WU を起こす（原因）

1. `crates/task-dispatch/src/dispatcher.rs` 2006 行: 毎 tick の `dispatch_ready` の最後に `dispatch_parallel_work_units` を呼ぶ。
2. `crates/task-dispatch/src/dispatcher/work_units.rs` 1823–1825 行の doc comment は「このインスタンスが既に run を持っている
   （＝工程の lease の持ち主の）Task だけを対象にする」と定める。実装の 1840–1846 行は
   `running_tasks_with_runnable_work_units` の task のうち `self.supervisor.runs_for_task(task.id) == 0` でないものを対象にする。
   planner の run も `runs_for_task` に数えられる（`crates/task-dispatch/src/run_supervisor.rs` 165 行）ので、「run を持つ ＝ 工程の
   lease の持ち主」は planner の run のときに成り立たない。
3. `crates/task-core/src/store/execution/work_units.rs` 466–487 行の `running_tasks_with_runnable_work_units` は、`running` の
   task のうち phase のある `ready`/`needs_continuation` の WU（kind が integrate・task 以外）を持つものを返す。lease は見ない。
   risk-rule（kind implement、stage impl）はここに当たった。
4. `crates/task-dispatch/src/dispatcher/dispatch_run.rs` 45 行で `forced_wu` があると `second_pass`。602 行
   `let task_lease = second_pass || { … }` で task の lease を取らず、`acquire_work_unit_lease` だけを呼ぶ。
   `crates/task-core/src/store/execution/work_units.rs` 364 行の確認は task が `running` であることだけで、lease の保持者を見ない。
   `crates/task-dispatch/src/dispatch_admission.rs` 541 行 `if first && fact(&input.task_runs, …)? > 0` も 2 本目以降では
   「同じ task の run」の gate を飛ばす（ADR 2026-11-14-dispatch-admission-order の「2 本目以降が飛ばす gate」）。
5. 次の tick の `crates/task-dispatch/src/dispatcher.rs` 1727 行 `abort_stale_runs`（`crates/task-dispatch/src/dispatcher/leases.rs`
   512–541 行）が `run_holds_lease`（`leases.rs` 738–755 行）で判定する。task の lease の `worker_run_id` は planner の run id で、
   WU の run id と違い、`phase:` で始まらない（`dispatcher.rs` 359 行 `is_phase_lease_holder`）ので `false`。WU の run を止めて
   `aborted (task no longer running under this lease)` で閉じ、`mark_just_aborted` する。WU は `needs_continuation` に戻る。
6. `mark_just_aborted` はその tick だけ効く（`work_units.rs` 1844 行 `was_just_aborted`。`abort_stale_runs` の先頭〈`leases.rs` 513 行〉で `clear_just_aborted`）。
   次の tick では 2.〜5. が繰り返される。WU の run の起動から打ち切りまでの約 3 秒と、再 dispatch までの約 4〜8 秒は、
   この tick の間隔と run の起動の時間に対応する（間隔の値そのものは本番の設定で確かめていない。未確認）。

`run_holds_lease` の判定（lease の持ち主は 1 つ。ADR 2026-10-03-ownerless-running-runs）は正しい。誤りは 2 本目以降を
選ぶ側にある。

2 本目以降の経路は、1 本目の `wu_dispatch_gate`（`work_units.rs` 412 行〜）も通らない。1 本目なら、人の replan の後は
`RunPlanner`（427–440 行）を返し、承認待ちの版（`task_ops::plan_gate::plan_gate_state` が `Pending`。`crates/task-ops/src/plan_gate.rs`
484–487 行・511 行）の unit は `human_gate_hold`（`work_units.rs` 445–466 行）で止める。今回の risk-rule は、承認されていない
v2 の unit なのに 2 本目以降の経路で起動された。

### (b) 承認前の版で WU を ready にする（前提。設計どおり）

- `crates/task-dispatch/src/health_review/design_verdict.rs` 131–139 行: 設計 review の不合格が `MAX_DESIGN_REVIEW_FAILURES`（2）回
  続くと、`adopt_reviewed_plan`（154 行〜）で計画を採用し、`design_review_failed:<n>` で人の承認に回す。
- 採用は `task_ops::execution::plan_replan`（`crates/task-ops/src/execution.rs` 568 行〜）。未完了で新しい版にも残る key は
  依存がすべて done なら `ready`（725–737 行、811 行で行に書く）、段のある計画では段の障壁つきで決め直す（1003–1036 行。
  reason `replan v{new_version}` の event は 1028–1035 行）。seq 1088 はここで積まれた。
- 承認されていない版の unit を起こさないことは、dispatch の時点の `human_gate_hold` が守る設計（ADR-0079 付記「R6-1」D1、
  既存の試験 `crates/task-dispatch/src/dispatcher/tests/human_gates.rs` 121 行〜
  `plan_gate_replan_does_not_dispatch_the_unapproved_version`）。採用の時点で unit を止める設計ではない。

判定: (b) だけでは事象は起きない（task は承認待ちの間 `blocked`、replan の後の `ready` では `wu_dispatch_gate` が planner を
選ぶ）。(a) が planner の lease の下で unit を起こしたことが原因。(b) を変えると採用・承認・replan の既存の挙動と試験の
期待値が変わるので、変えない。(a) を直せば、planner が走る間は unit を起こさず、planner の後は新しく採用された版で unit の
状態が決め直される（seq 1867 の `replan v3` と同じ経路）。

## 修正の方針（WorkUnit fix-test が実装する）

- owner: `crates/task-dispatch/src/dispatcher/work_units.rs` の `dispatch_parallel_work_units`（1831 行〜）。
- 変更: 1840–1846 行の判定に、`task.lease` が有り、その `worker_run_id` が `is_phase_lease_holder`（`dispatcher.rs` 359 行。
  `run_holds_lease` と同じ述語）を満たすことを足す。満たさなければその task を飛ばす。`runs_for_task > 0`・`is_integrating`・
  `was_just_aborted`・`halted_by_pause` の判定は今のまま。doc comment の「＝工程の lease の持ち主の」を、この判定で
  確かめると書き直す。
- 変えないもの: `plan_replan` の状態の決め方、設計 review の不合格の後の採用と PlanGate、`human_gate_hold`、
  `run_holds_lease`、`acquire_work_unit_lease`（store）、`dispatch_admission::admit`。
- 新しい状態・分岐・fallback・設定: 無し（既存の述語を 1 つ使う判定を足すだけ）。
- 撤去対象: 無し（置き換える仕組みは無い。「run を持つ ＝ 工程の lease の持ち主」という doc comment の近似を正す）。
- 高 risk の path class: 変える file は `crates/task-dispatch/src/dispatcher/work_units.rs` で、`lease` class の正規表現
  （`dispatcher/leases\.rs`・`ownerless_runs\.rs` ほか）には当たらない。`leases.rs` は変えない。

## 試験案（反例試験と失敗注入。WorkUnit fix-test）

守る不変条件: task の lease を task 単位の run（planner）が持つ間は、その task の WU の run を起こさない。

置き場所: `crates/task-dispatch/src/dispatcher/tests/human_gates.rs`（既存の D1 の試験の隣）。tree の helper
（`tree_dispatcher`・`v3_plan`・`stage`・`leaf`・`task_unit`・`approve_root_plan`）を使う。

再現の手順（出来事待ち。sleep・CPU の負荷・壁時計の経過に頼らない。tick は試験が `d.tick()` で進める）:

1. adapter: planner の run は、計画を書いた後、試験が開けるまで `tokio::sync::Semaphore` で止まる
   （`crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` 13–60 行の `GatedAdapter` と同じ形。1 回目の planner の
   run は止めずに返す）。planner 以外の run は起動した run id を記録して `Done` を返す。
2. v1 = 段 s1 に kind task の unit c と依存の無い leaf a（kind implement、phase 有り）を入れ、root の承認待ち
   （`PlanGateState::Pending`）まで `run_until_quiet` で進める。leaf a が `ready` であることを確かめる（承認待ちの版の
   `ready` の unit。本番の risk-rule と同じ立場）。
3. `task_ops::plan_gate::plan_gate(…, PlanGateAction::Replan, …)` で答える。
4. `d.tick()` と `tokio::task::yield_now()` を、planner の `WorkerStarted{role: Planner}` が events に出るまで繰り返す
   （回数の上限を置く）。task が `running`、lease の保持者がその planner の run id であることを確かめる。
5. さらに 5 tick 進め、その間に (i) leaf a の `WorkUnitTransitioned{to: running}` が無い、(ii) `aborted (task no longer running
   under this lease)` の `WorkerFinished` が無い、(iii) leaf a の `runs` が 0、(iv) lease の保持者が planner の run id のまま、
   を確かめる。
6. planner を開け、次の版を `approve_root_plan` で承認し、`run_until_idle` で task が `done` まで進むこと、leaf a が通常どおり
   1 回 dispatch されることを確かめる（通常の dispatch を変えていないこと）。

既存の試験で検出できない理由: `plan_gate_replan_does_not_dispatch_the_unapproved_version` は、(1) planner の adapter がすぐ
返すので planner の run の間に別の tick の `dispatch_parallel_work_units` が回らない、(2) 承認待ちの v1 の unit は kind `task`
で、`running_tasks_with_runnable_work_units` の対象外。

失敗注入: 修正の判定（`is_phase_lease_holder` の確認）を外して同じ試験を回し、5. の (i) で落ちること（leaf a が planner の
lease の下で `running` になる）を確かめ、出力の要点をこの進捗に書く。

回す試験（nextest、全体の試験は走らせない）: 足した反例試験に加え、計画の承認・replan・lease に関わる task-dispatch の試験
を filter で回す。候補: `human_gates`・`tree_approval`・`tree_replan`・`work_units::`（並列の試験）・`cex_lease_restart`・
`ownerless_runs`・`orphan_work_units`・`dispatch_admission`。filter と件数は fix-test が書く。

## 変更
- `planner-lease root-cause: ADR 2026-11-18 task 単位の run が task の lease を持つ間は WU を起こさない（原因 (a)・(b) は変えない・反例試験）`
  — ADR を足した（実装の前の commit）。
- `planner-lease root-cause: 進捗に原因（events の seq とコードの行）・修正の方針・試験案を書く` — この file。
- `planner-lease fix-and-test: planner の lease の間は並列 WU を起こさない・反例試験・失敗注入`（fix-and-test）
  — `dispatch_parallel_work_units`（`crates/task-dispatch/src/dispatcher/work_units.rs`）が、task の lease の保持者が工程
  （`is_phase_lease_holder`）の task だけを対象にする。doc comment を直した。旧い近似の経路・fallback・切替 flag・unwrap は無い。
  `crates/task-dispatch/src/dispatcher/tests/human_gates.rs` に `planner_lease_blocks_parallel_work_unit_dispatch` を足した。
- source-size gate（file の production 行 2060 以下）のため、同じ関数の `halted_by_pause` の判定を条件に併合し、`in_flight` の
  数え方と `tried` の更新を畳んだ（挙動は変えない）。

## 証拠（WorkUnit root-cause。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" "select seq, ts, substr(json,1,330) from events where task_id='01M7MZTKB29VF5TX7FCQ15XEMW' and seq between 1060 and 1135 order by seq"`
  → seq 1087–1135 の流れ（上の表）。
- 同じ DB で risk-rule の `dispatch` を数えた（seq 1088–1676）→ 56 件（seq 1102–1664）。
  `aborted (task no longer running under this lease)` の `worker_finished` → seq 1088–1676 で 55 件（1109–1662、usage null 55 件）、
  task 全体で 57 件。
- 同じ DB で 2026-11-01 以降の同じ打ち切りを task ごとに数えた → 01M6TYVJT0Z1CBGNK8VTNK8ZJH 30 件（11-10 23:38Z から）、
  01M7MZTKB29VF5TX7FCQ15XEMW 57 件だけ。
- 同じ DB で 2026-11-01 以降の `plan_gate_decided` action replan → 3 件（上の 2 task と 01M6TYVJT4S29ZMP6P0D4831JG）。
- 検査（`bash scripts/dev/test-affected.sh`・`cargo clippy`・`cargo fmt`）: この WorkUnit はコードを変えないので走らせていない。
  fix-test が提出時の HEAD で走らせる。

## 結果（WorkUnit fix-and-test。提出時の HEAD で実行）

反例試験 `dispatcher::tests::human_gates::planner_lease_blocks_parallel_work_unit_dispatch`:
- 守る不変条件: task の lease を task 単位の run（planner）が持つ間は、その task の WU の run を起こさない。planner の後は
  承認された版で leaf a が 1 回だけ dispatch される。承認待ちの版の `ready` の leaf a は承認まで dispatch されない。
- 手順: v1（c = kind task、leaf a）を承認待ちにし、人が replan → 2 回目の planner の run を `Semaphore` で止める（出来事待ち。
  sleep・CPU 負荷なし）→ 5 tick 進め、leaf a の `running` 遷移なし・`aborted (task no longer running under this lease)` の
  `WorkerFinished` なし・leaf a の `runs` 0 かつ `ready`・lease の保持者が planner の run のまま、を確かめる。planner を開けた後も
  承認待ちのまま leaf a は動かず、承認後に task が `done`、leaf a の `running` 遷移がちょうど 1 回。
- 既存の試験で検出できない理由: `plan_gate_replan_does_not_dispatch_the_unapproved_version` は planner の adapter がすぐ返すので
  planner の run の間に別 tick が回らず、v1 の unit は kind task だけで 2 本目以降の経路の対象外。lease の保持者の判定を外しても
  通る。
- 試験の名前の通り、時計は進めていない（`tokio::time::pause` は不要。tick は試験が進める）。

失敗注入: 判定（`matches!(&task.lease, Some(l) if is_phase_lease_holder(..))`）の行だけを一時的に外して同じ試験を回した →
`1 test run: 0 passed, 1 failed`。panic: `leaf a must not start under the planner's lease` で、新しい event は
`WorkerFinished` 「interrupted: aborted (task no longer running under this lease)」→ leaf a `Running→Ready` reason `aborted`
→ `Ready→Running` reason `dispatch` → また `aborted` を、5 tick の間に 3 回繰り返した（本番の事象と同じ形）。
判定を戻した後の diff に判定の行が残ることを `git diff` で確かめ、試験が通ること（1 passed）を確かめた。

検査（提出時の HEAD。基準は `$CELERIS_WU_BASE`。全体の試験は走らせていない）:
- `cargo nextest run -p task-dispatch human_gates work_units parallel cex_lease ownerless orphan` → 115 passed、0 failed。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `python3 scripts/dev/check-source-size.py ...`（test-affected の source-size 検査と同じ）→ 0 failure（最初の実装は
  `work_units.rs` が 2066 行で落ちたので上の畳みで 2060 以下にした）。
- `bash scripts/dev/test-affected.sh` の結果は完了報告に書く。

本番への移行: 無し（config・DB・API の変更なし）。

## 未解決
- seq 938–942（03:21:29Z〜03:21:32Z）: 設計 review の判定（seq 936）の後に起動した reviewer の run が約 3 秒で
  `aborted (task no longer running under this lease)` になった。この事象とは別の経路で、原因は調べていない（未調査）。
- 2 本目以降の経路は `human_gate_hold` を通らない。修正の後は工程の lease の下でしか 2 本目以降は起きず、工程の lease は
  1 本目が `human_gate_hold` を通った後に取る。工程の lease の下で人の gate が新たに `Pending` になる経路があるかは確かめて
  いない（未確認。PlanGate の遷移は `ready` から）。
- 打ち切った 56 本の run の usage は null。claude の session の起動だけで token が使われたかは確かめていない（未確認）。

## 提案
- `running_tasks_with_runnable_work_units` の doc と `dispatch_parallel_work_units` の doc に、「工程の lease の下の task だけ」を
  前提として明記する（fix-test で doc comment を直すときに合わせる）。
