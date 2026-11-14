---
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: accepted
---
# ADR 2026-11-14: dispatch gate は計画の版によらず、依存の状態から WU を ready に上げる

- 状態: 採用（実装は task 01M6TYVJT4S29ZMP6P0D4831JG の WorkUnit fix-promote）
- 関係: [ADR 2026-11-11 Dispatcher の状態の owner と、判断と副作用の分け方](2026-11-11-dispatcher-state-ownership.md) の
  D7（今の実装で成り立たない反例は owner で直す）を、cex-lease の未解決の反例について具体にする。ADR-0072 D15（依存の解決）と
  ADR-0079 付記「R6-1」D2（gate が解けた後の dispatch で上げる）の規則は変えず、適用する計画を v1 にも広げる。
  ADR 2026-11-11 の本文は変えない。

## 背景

`crates/task-dispatch/src/dispatcher/worker_finish.rs` の `finish_worker_result_with` は、WU a の done
（`work_unit_transition_with_events`）と、依存 WU の pending → ready（`side_effect_rows` ごとの `work_unit_transition`、
reason `dependency_ready`）と、Task の遷移・`runs` の閉じ・`WorkerFinished` を別々の書き込みにしている。

WU a の done の直後で daemon が落ちると、再起動の後に新しい active が run A を回収して Task を ready に戻す。ところが
`crates/task-dispatch/src/dispatcher/work_units.rs` の dispatch gate は、依存の状態からの昇格（`promote_newly_ready`）を
v2 の計画（段のある計画）だけで行っていた。v1 の計画では b・c が pending のまま残り、`task_core::next_work_unit` は
`Stuck` を返す。失敗した WU も依存の失敗も無いので replan にも回らず、gate は `Skip` を返し続けて Task が進まない
（`agent-docs/progress/2026-11-11-health-v2-r4/cex-lease.md` の未解決、最終 review の criterion 1）。

## 決定

### D1. ready は SQLite の依存の状態から導く値とし、dispatch gate はどの版の計画でも昇格させる

pending の WU を ready に上げてよいかは、store の WU 行（依存先の状態）だけから `task_core::newly_ready` で決まる。
行と event は `phase_integration::ready_promotions` が作る。v2 の dispatch gate・`finish_phase_integration`・
`reconcile_tree_units` は既にこの関数で上げている。

`Dispatcher::wu_dispatch_gate` の `promote_newly_ready` の呼び出しから、計画の版の条件を外す。v1 の計画でも、Ready の Task の
dispatch のたびに依存の満たされた pending の WU を上げる。finish の側の昇格（`execution_scheduler` の `complete` が同じ
`newly_ready` で求める行）は残す。finish の側の昇格は WU a の done と同じ tick に次の WU を起こすためのもので、正本ではない。
どちらも pending の行だけを上げるので、同じ WU に `dependency_ready` が 2 回書かれることはない。

`reopen_stale_stage_integrations` は段のある計画だけが持つ統合 WU を扱うので、v2 の計画だけで呼ぶ（変えない）。

### D2. v1 の計画で gate と finish の側が上げる集合は同じ

finish の側は「WU a を done にした行に置き換えた WU の集合」を `newly_ready` に渡す。gate は WU a の done を書いた後の store の
WU 行を渡す。両者の違いは a の `updated_at`・`last_run_id` だけで、`newly_ready` の判定（`wait::UnitWaitJudge`）はこれらを
読まない。v1 の計画には統合 WU も段も無い。finish の側が昇格を取り消す 2 つの場合（`hold_for_auto_leaf`・
`hold_for_worker_decisions`）は、a を done にせず `blocked(decision)` にするので、store の行からも a の依存先は上がらない。
`child:`・`task:` の依存は gate では `resolve_child_dependencies` が先に解き、`newly_ready` はどちらの側でも外部の完了を
空の集合として読む。

### D3. 選ばなかった案: WU の done と依存 WU の昇格を 1 つの transaction にする

`TaskStore::work_units_apply`（挿入なし）を使えば、WU a の done と依存 WU の昇格を 1 回で書ける。この案は採らない。

- 1 回にしても、Task の遷移・`runs` の閉じ・`WorkerFinished` はその後の別の書き込みのままで、WU の書き込みの直後に落ちた
  ときの前進は、再起動の後の回収と dispatch gate に頼る。gate が依存の状態から導けば、WU の行どうしの間の窓は追加の修正なしに
  閉じる。
- finish の書き込みを 1 回にまとめるには、`finish_worker_result_with` の書き込みの順（WU の自動 commit、write set の記録、
  決定の要求の行）を組み直す必要があり、この反例の修正の範囲を超える。
- 「finish の側の書き込みが成功したこと」を ready の正本にすると、正本が 2 つになる（store の依存の状態と、finish が書いたか
  どうか）。D1 は正本を store の依存の状態の 1 つにする。

## 試験

- `crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` の `cex_restart_after_wu_done_promotes_dependents_once`:
  v1 の計画 a → b → c で、`test_hooks.halt_after_wu_transition` により WU a の done の直後で finish を止め、新しい active で
  再起動する。b・c が `dependency_ready` で 1 回ずつ ready に上がり、Task が Done に届き、a を走らせ直さず、もう一度再起動しても
  event が増えないことを確かめる。修正の前は Task が ready のまま `run_until_state` の保険（60 秒）まで進まずに落ちる。
