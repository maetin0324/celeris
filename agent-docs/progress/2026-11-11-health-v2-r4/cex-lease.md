---
title: "健全性v2 R4 cex-lease: lease・restart・同時完了の反例試験"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: completed
updated: 2026-11-11
---
# 健全性v2 R4 cex-lease: lease・restart・同時完了の反例試験

完了日: 2026-11-11。基準は `$CELERIS_WU_BASE`。本体（dispatcher・store）は変えていない。

## 変更

- `phase 3: pin lease, restart and simultaneous-finish counterexample tests (cex-lease)` — `crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` を足し、`tests/mod.rs` に `mod cex_lease_restart;` を 1 行足した。
- `phase 3: strengthen the draining counterexample and pin the holder re-close of a finished run` — draining の試験に backoff の無い別 task を足し、持ち主の閉じ直しの試験を 1 本足した。

試験は Dispatcher の private field に触らず、store の行（task・lease・WU・`runs` 索引）と event、`tick()` の戻り値（`TickReport`）だけを見る。待ちは `run_until_state` と `TickReport.in_flight` の出来事待ち（保険は壁時計の `STATE_WAIT_GUARD`）で、tick 回数の上限で待たない。run の終わりは fake adapter の gate（semaphore・`Notify`）で試験が決め、lease の失効は `renew_lease(.., 0)` で store の lease 時刻を今にして作る。CPU 負荷は使わない（検査の待ちは `sleep 0.05` の繰り返し）。

## 作業前 brief

1. 根本原因: R4 の分割（RunSupervisor・run_settlement など）の前に、lease・restart・同時完了の不変条件を固定する試験が無い（cm-test-gaps の「反例試験の不足」）。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `leases.rs`（reclaim・abort_stale_runs・close_aborted_run）、`worker_finish.rs`（on_worker_finished の lease 照合）、`orphan.rs`（holder_gone）、`dispatcher.rs`（drain_completions・draining の dispatch 停止）。
3. 再利用: `orphan_takeover.rs` の `running_orphan`・`new_active`・`instance`・`slow_adapter`、`tests/mod.rs` の `ParallelWuAdapter`・`parallel_dispatcher`・`adopt_v2_plan`・`adopt_three_step_plan`・`run_until_state`。
4. 選択肢: 試験専用 hook（`test_halt_after_wu_transition`）は private field なので使わず、落ちた状態は store に直接作った。2 instance の競合は同じ store に dispatcher を 2〜3 個作って再現した。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の表の各行。

## 試験（9 本、全部合格）

| 試験 | 守る不変条件 |
|---|---|
| `cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run` | lease を回収された旧持ち主 A の遅れた完了が、再割当の run B の lease・状態・attempts・runs 行を書き換えない |
| `cex_orphan_takeover_leaves_the_run_held_in_this_instance` | 他の instance が無くても、自分の手元の run は lease が残る間は孤児として回収しない |
| `cex_orphan_takeover_leaves_a_run_whose_active_holder_lives` | 生きている別の active instance が持つ run を横取りしない。持ち主が消えた後に 1 回だけ回収する |
| `cex_draining_instance_reclaims_its_expired_lease_without_redispatching` | draining の instance は自分の run の切れた lease を 1 回だけ回収し、draining の間は新しい run を起こさない |
| `cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` | WU done の後・Task 遷移の前に落ちた状態から再起動しても、A を 1 回だけ閉じ、WU a を走らせ直さず、2 回目の再起動で何も足さない |
| `cex_reclosing_a_run_after_worker_finished_does_not_write_twice` | `WorkerFinished` 記録後・WU 回収前に落ちた run を再起動後に閉じ直しても二重に書かず、WU を回収して Task を完了させる |
| `cex_holder_reclosing_an_already_finished_run_does_not_write_twice` | 別の経路が先に `WorkerFinished` を書いた手元の run を持ち主が閉じ直しても二重に書かず、WU の回収を省かない |
| `cex_two_parallel_work_unit_runs_finishing_in_one_tick_lose_no_update` | 並列 WU の run 2 本の完了を同じ tick（`TickReport.finished == 2`）で受けても、両 WU の done・`WorkerFinished`・runs 行が失われず、replay と一致する |
| `cex_run_finish_and_work_unit_check_finish_in_one_tick` | WU a の検査の完了と WU b の run の完了を同じ tick で受けても、両方 done、`WorkerFinished` は各 1 件、統合は 1 回 |

各試験の doc comment に不変条件と、既存の試験で検出できない理由を書いた。

## guard を一時的に外すと落ちることの確認

提出時の HEAD に、本体の 1〜2 行を一時的に書き換えて `cargo nextest run -p task-dispatch -E 'test(/cex_/)' --no-fail-fast` を実行した。一時変更は `git checkout --` で戻し、commit していない（戻した後に `grep -r GUARD-OFF crates/` が 0 件、`git status` は試験 file だけ）。2 回の build に分けた。

| guard（一時変更） | 落ちた試験（assert） |
|---|---|
| G1 `worker_finish.rs` `on_worker_finished`: `run_holds_lease` の結果を常に true | `cex_stale_holder_late_finish_…`（`assert_eq!(t.status, Status::Running)`）。A の遅れた完了が B の Task を遷移させた |
| G2 `leases.rs` `reclaim_expired_leases`: `|| self.holds_task_in_hand(task.id)` を削除 | `cex_orphan_takeover_leaves_the_run_held_in_this_instance`（`report.reclaimed == 0`）。自分の run を孤児として回収した |
| G3 `leases.rs` `close_aborted_run`: `!already &&` を削除 | `cex_holder_reclosing_an_already_finished_run_…`（`finishes(..).len() == 1`）。`WorkerFinished` が 2 件になった |
| G4 `orphan.rs` `holder_gone`: active の instance を持ち主に数えない | `cex_orphan_takeover_leaves_a_run_whose_active_holder_lives`（`report.reclaimed == 0`） |
| G5 `dispatcher.rs` `tick`: `dispatch_ready` の `self.accepting_new_work &&` を削除 | `cex_draining_instance_…`（`(report.dispatched, report.in_flight) == (0, 0)`）。draining の instance が別の task を起こした |
| G6 `dispatcher.rs` `drain_completions`: `while let` を `if let`（1 tick に 1 件） | `cex_two_parallel_work_unit_runs_…` と `cex_run_finish_and_work_unit_check_finish_…`（`report.finished == 2`） |
| G7 `phase_integration.rs` `reconcile_work_unit_run`: running の条件を「pending 以外」に緩める | 落ちない。`cex_restart_after_crash_…` は、store の `recover_work_unit` の比較更新（running の行だけを戻す）でも守られる |

build A（G1・G3・G4・G5）: 9 本中 4 本が失敗、5 本が合格。build B（G2・G6・G7）: 9 本中 3 本が失敗、6 本が合格。guard を戻した後は 9 本すべて合格。

G7 について: `cex_restart_after_crash_…` を落とす guard は dispatcher 側と store 側（`crates/task-core/src/store/` の `recover_work_unit`）の 2 重で、dispatcher 側だけを外しても落ちない。store 側は高 risk の path class なので一時変更でも触っていない。この試験の guard 確認は未完了。

`cex_reclosing_a_run_after_worker_finished_…`（再起動の閉じ直し）の guard 確認も未完了。store は `WorkerFinished` の追記と同じ transaction で runs 行を閉じる（`close_run_row_for_event_tx`）ので、再起動の後に `close_aborted_run` は通らず、WU は `reconcile_ownerless_work_units` が戻す。この試験は二重に書く経路が無いことと、WU の回収が省かれないことを固定する。`already` の guard は `cex_holder_reclosing_…` で確かめた。

## 未解決

- 反例（今の実装で成り立たない）: WU a を done にした書き込みの直後（`finish_worker_result_with` の WU 遷移の後、依存 WU の `dependency_ready` 遷移の前。`test_halt_after_wu_transition` と同じ所）で落ちると、再起動後に b・c が pending のまま残り、Task は ready（lease 無し）で止まる。`run_until_state` の保険まで Done に届かない。観測した event: `work_unit_transitioned a running→done (completed)`、`worker_progress orphan_takeover…`、`transitioned running→ready (infra_requeue)`、`worker_finished … harness_error(infra)` の後に何も起きない。再現: `cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent` の最後に `assert!(run_until_state(&mut again, || status_of(&s, id) == Status::Done).await)` を足す。今回の範囲では本体を直さず、試験はこの前進を assert していない（doc comment に明記）。
- G7 と再起動の閉じ直しの guard 確認は上の理由で未完了。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cex_/)' --no-fail-fast`: 9 本すべて合格、exit 0。
- guard の確認は上の表のとおり（一時変更は commit していない）。
- `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、`bash scripts/dev/test-affected.sh` の結果は run の報告に書く。

## 提案

- 上の反例は、R4 の `settle`（run の終わりの判断を run_settlement へ分ける）で、WU の done・依存 WU の昇格・Task の遷移・runs の閉じを 1 つの原子更新にするか、再起動の照合が「依存が全部 done の pending WU」を昇格させるようにして直す。どちらにするかは settle の設計で決める。直すときに上の assert を試験に戻す。
- `reclaim_expired_leases` と `reconcile_ownerless_runs` が同じ「持ち主が居ない」判断を別々に持つ（`holds_task_in_hand` と `holds_run_in_hand`）。own-supervisor で RunSupervisor に移すとき、G2・G4 の試験がそのまま回帰検査になる。
