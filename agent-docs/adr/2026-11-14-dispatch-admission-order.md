---
tasks: [01M7BYQNFCE4Z8PFD95T6VQ37J]
status: accepted
---
# ADR 2026-11-14: run を起こすかの順序と組み合わせを `dispatch_admission::admit` に置く

- 状態: 採用（実装は task 01M7BYQNFCE4Z8PFD95T6VQ37J の WorkUnit admit-gate・admit-place・admit-launch。admit-gate は
  capacity から browser の前提までを移した）
- 関係: [ADR 2026-11-11 Dispatcher の状態の owner と、判断と副作用の分け方](2026-11-11-dispatcher-state-ownership.md) の
  D2（owner の規則）・D3（dispatch_one の行）・D5（接続の契約）・D8（budget の移動先）を、dispatch_one について具体にする。
  ADR 2026-11-11 の本文は変えない。

## 背景

`crates/task-dispatch/src/dispatcher/dispatch_run.rs` の `dispatch_one` は、run を起こすかの判断（どの gate をどの順で見るか、
並列 WU の 2 本目以降はどの gate を飛ばすか、planner run にするか）と、その途中の副作用（待ちの記録・遷移・担当の書き戻し・
lease・spawn）を 1 つの関数の中で交互に行う。ADR 2026-11-11 D3 は、判断を
`dispatch_admission::admit(AdmissionInput) -> AdmissionProposal` の純粋関数に分け、個々の判定は既存の関数の結果を
組み合わせるだけにすると決めた。

gate の多くは副作用を持つ既存の関数（`assign_if_needed` は担当を書き戻すか task を blocked にする、`browser_prereq_hold` は
task を遷移させる、`design_review_hold` は設計 review の run を起こすか判定を記録する）で、今は前の gate が止めたら後ろの関数を呼ばない。
すべての fact を先に集めてから判断すると、今は呼ばれない副作用が起き、順序が変わる。

## 決定

### D1 入出力

- `admit(&AdmissionInput) -> AdmissionProposal` は入力の値だけを読む純粋関数である。永続化・file・process・`Dispatcher` に
  触れず、`dispatcher` module を使わず、log も出さない（ADR 2026-11-11 D2 の 2・3）。
- `AdmissionInput` は dispatch_one がそれまでに集めた fact を持つ。欄の `None`（bool の欄は `false`）は「まだ集めていない」を表す。
  並列 WU の 2 本目以降は `forced_unit`（dispatch_one が受けた WU を落とした値）が `Some` になる。
- `AdmissionProposal` は次の 3 つのどれかである。
  - `Need(AdmissionFact)`: 次に集める fact の要求。
  - `Stop(AdmissionStop)`: 起こさない。dispatch_one が止め方を適用して `Ok(false)` を返す。
  - `Launch(AdmissionLaunch)`: 起動へ進む。dispatch_one が lease 以降を今のとおり行う。
- `AdmissionInput`・`AdmissionProposal`・`admit` は根の file `crates/task-dispatch/src/dispatch_admission.rs` に置き、
  試験は `crates/task-dispatch/src/dispatch_admission/tests.rs` に置く。呼ぶのは `dispatch_run.rs` の
  `dispatch_admission::admit(` だけである。

### D2 fact の要求の形

- `AdmissionFact` の 1 つの variant が、既存の 1 つの判定か owner の 1 つの処理を指す。引数のうち admit が決める値
  （CoS の枠で数えるか、planner run か）は variant が持つ。
- dispatch_one は要求された variant に対応する既存の関数を呼び、結果を `AdmissionInput` の対応する欄に入れて、もう一度 admit を呼ぶ。
  dispatch_one は結果を組み合わせず、次に何を集めるかも決めない。
- 要求のうち次のものは、owner の既存の処理を今と同じ位置と条件で走らせ、その結果を fact として返す（副作用を含む）。
  担当の決定（`assign_if_needed`）、execution gate（`execution_gate_if_needed` の後に `apply_plan_invalid_atomic`）、
  WU の gate（`wu_dispatch_gate`）、決定の待ち（`decision_self_hold`）、設計 review の待ち（`design_review_hold`）、
  期限を過ぎた infra backoff の解除（`clear_infra_backoff`）、browser の前提の保留（`browser_prereq_hold`）。
- dispatcher module の非公開の型（`WuDispatchGate`）は dispatch_one が admit の要る値（`AdmissionUnit`）に落として渡す。
  `tree_units::next_run` のように dispatcher の型を受ける既存の関数は dispatch_one が呼ぶ。

### D3 止め方と起動

- `AdmissionStop` は、待ちを記録しない見送り、既存の `TaskWait` を 1 件記録する見送り、backoff の待ち（期限と原因。文字列化と
  debug log は dispatch_one）、段の統合の開始（`start_integration_from_ready`）、最終 review の開始（`start_final_review_from_ready`）の
  どれかである。
- `AdmissionLaunch` は、planner run か・replan か・人が frontier を明示した planner run か・run の tier と予算の上書きを持つ。
  dispatch_one は上書きを task に当ててから write-set の gate 以降へ進む。

### D4 副作用は dispatch_one に残す

待ちの記録・遷移・接続・作業場所・route の記録・credential の待ち・lease・event・spawn は dispatch_one（と、それが呼ぶ owner の
method）に残す。admit が要求したとき、または止め方・起動として返したときだけ、今の順で行う。

### D5 守る順序（admit-gate の範囲）

admit は次の順で fact を求め、最初に止めた gate で止める。「1 本目だけ」は並列 WU の 2 本目以降では fact を求めずに通す。

1. 知識整理 run の入力が揃っているか（`task_ops::knowledge_curation::inputs_ready`）。
2. 同じ task の run が走っていないか（1 本目だけ。`RunSupervisor::runs_for_task`）。
3. 並列度の枠（`RunLoad::admits`。CoS の対話 run かは 1 本目だけ `is_cos_task` で調べ、2 本目以降は CoS でないとする）。止めたら
   `TaskWait::Capacity`（scope は CoS か全体）。
4. この tick で打ち切ったばかりの run が無いか（`was_just_aborted`）。
5. この instance が触ってよい task か（`is_eligible`）。
6. disk が critical か（`held_by_disk_critical`）。止めたら監視 path（`disk_watch_paths`）を載せた `TaskWait::DiskCritical`。
7. 担当の決定（1 本目だけ）。候補が無く blocked にしたら止める。
8. execution gate と `plan_invalid` の atomic（1 本目だけ）。
9. WU の gate（1 本目だけ `wu_dispatch_gate`。2 本目以降は受けた WU）。
10. 決定の待ち、続けて設計 review の待ち（1 本目だけ。決定の待ちが止めたら設計 review の待ちを求めない）。
11. 木の上限（`tree_units::next_run` が run を返す gate だけ `tree_limit_hold_for`）。
12. WU の gate の種類（段の統合・最終 review・何もしないなら止める）。
13. planner run かの組み合わせ（replan、または計画の無い run で、直行経路でなく、gate の判定が compound で、`[execution] gate` が on か、
    shadow で人が明示したか、木の子 task であるか）と予算の上書き（planner run は `[execution.planner]`、WU の run は WU の予算か
    ADR-0072 D18 の既定）。
14. 再試行の backoff（1 本目で attempts が 1 以上のときだけ）。
15. infra の backoff（1 本目だけ。期限の前なら止め、過ぎていれば解除を求めてから進む）。
16. browser の前提（1 本目で `browser_prereq_applies` が真のときだけ `browser_prereq_hold`）。

予算の上書きを task に当てる位置は、14〜16 の後（`Launch` を受けた dispatch_one）に移した。14〜16 の判定と止め方が読む task の値は
id・attempts・更新時刻・種類・skills・requirements だけで、tier と予算を読まない。止めたときは上書きした task を捨てるので、
当てる位置を後ろにしても結果は同じである。

admit-place と admit-launch は、write-set・クラスタ・作業場所・disk の見込み、lane の保留・route・credential・variant の pin を
同じ形（`AdmissionFact` の variant と `AdmissionStop` の止め方）で 16 の後ろに足す。

### D6 試験

- `dispatch_admission/tests.rs` は、隣り合う 2 つの gate が両方止める入力で今の順の先が勝つこと、2 本目の WU が飛ばす gate の集合、
  planner run の組み合わせを、admit を直接呼んで確かめる。
- 既存の dispatcher の試験は 1 つの試験で 1 つの gate だけを止めるので、隣り合う gate の順を入れ替えても合格する。
  入れ替えはこの試験でだけ検出できる。
- ADR 2026-11-11 の試験と `cex_` の試験と既存の試験は変えない。

## 不採用

- すべての fact を先に集めてから admit を 1 回呼ぶ案。今は前の gate が止めたら呼ばない副作用（担当の書き戻し、blocked、
  設計 review の run の起動、browser の前提の遷移）が起き、順序が変わる。
- admit が fact を集める trait や closure を受けて自分で呼ぶ案。admit が副作用を持つ関数を呼ぶことになり、純粋でなくなる。
- admit が `&Task` と設定を受けて gate の関数を自分で呼ぶ案。`dispatcher` の非公開の型と `Dispatcher` を受ける関数を使えず、
  個々の判定を写すことになる。

## 戻し方

unit ごとの commit を `git revert` で戻す。新旧を切り替える flag・互換の分岐は置かない（ADR 2026-10-11 破壊的変更）。
