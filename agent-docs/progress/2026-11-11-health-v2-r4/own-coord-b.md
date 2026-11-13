---
title: "健全性v2 R4 own-coord-b: task・WU の待ち・check と統合の process・tick の field を ExecutionCoordination へ移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: running
updated: 2026-11-13
---
# 健全性v2 R4 own-coord-b

## 概要（Run 1 の時点。未完了）

- ADR 2026-11-11 dispatcher-state-ownership D1 の表で unit 列が own-coord-b の 16 field（`accepting_new_work`・`awaiting_children`・
  `awaiting_human`・`checking`・`dispatch_waits`・`integrating`・`last_tick_gap_ms`・`last_tick_started`・`liveness_checked_at`・
  `max_concurrency`・`pending_subjects`・`rx`・`stall_watch`・`stalled_tasks`・`ticks`・`tx`）を `Dispatcher` から消し、
  `crates/task-dispatch/src/execution_coordination.rs` の `ExecutionCoordination` の private field に移した。D1 の表と実装は合っていた（ADR は変えていない）。
- review の持ち越しの値（`ReviewSubject`）と完了の channel の値（`Completion`）は dispatcher の型なので、owner は型引数
  `ExecutionCoordination<S, C>` で受ける（`RunSupervisor<S>` と同じ形。owner は review の module と `dispatcher` module を使わない）。
  値の型 `IntegrationEntry`・`CheckingEntry`・`AwaitingChildren` は同じ file に移し、旧い定義を消した。
- lease の回収・abort の待ちの削除（D3 の reclaim_expired_leases の行）: `abort_stale_runs` は `RunSupervisor::take_review` が返した鍵で
  `ExecutionCoordination::release_reclaimed_review` を呼ぶ。drain（`abort_all_runs`）と持ち主の喪失（`ownerless_runs.rs`）は
  `abort_in_hand`（check と統合を返し、持ち越しを消す）を通す。`holds_task_in_hand` は `RunSupervisor` の 2 つと `ExecutionCoordination::holds_task`。
  own-supervisor が `leases.rs` に残した field の直接の削除は無くなった。
- D6: stale 優先の ablation は `stale_priority.rs` の `stale_priority_disabled` が `TestHooks` から読み、`recover_reviews` が
  `order_review_sync_queue` の引数で渡し、`ExecutionCoordination::enter_review_sync_queue(candidates, priority_disabled)` が並べ替えるかを返す。
  `#[cfg(test)]` の分岐は判断の関数から消えた。`note_review_sync_offer` は owner が自分の `awaiting_human` を読む（bool の引数を消した）。
- tick の順序は変えていない（`begin_tick` で `count_tick`、その後 `script_tick_rollover`、その後 `note_tick_started(Instant::now())`）。
  停滞の見張りは `liveness_due`・`stall_since`・`forget_stall`・`finish_stall_round`・`clear_stall_watch` に置き換えただけで、通知の冪等性の判定は同じ。

## 未解決（次の run でやること）

- repo-health の `size.responsibility-growth`（new、fail）: `ExecutionCoordination` の method の数が 78 で閾値 60 を超える。
  base（`$CELERIS_WU_BASE`）では超えていない。直し方の候補: 停滞の見張り（3 field・7 method）と WU の check・段の統合の process
  （2 field・16 method）を、`ExecutionCoordination` の中の private な owner の struct（`QuotaBook` が `RoutingRuntime` の中にある形）に分け、
  `OWNER_STRUCTS` と structure guard を直す。method をまとめて数を減らす（`is_integrating` → `integration_work_unit().is_some()` など）案もある。
- 上の直しの後に、test-affected・clippy・fmt・repo-health・範囲の check を提出時の HEAD で走らせる。

## 試験の追加と変更

- 足した試験（`execution_coordination.rs`）:
  - `reclaimed_review_keys_release_only_their_waits`: `RunSupervisor` が手放した鍵の task の持ち越しだけが消え、他の task の持ち越しと
    check・統合の保持は残る。drain は check と統合を全部返して持ち越しを消す。dispatcher 経由の試験は手放した後の DB を見るので、
    鍵を無視して全部消す実装でも通る。
  - `stall_watch_keeps_the_first_sighting_until_the_seq_moves`: 同じ seq の間は最初に見た時刻を保ち、seq が変われば数え直す。
  - `structure_guard_execution_coordination`: field の一覧を 30 件（own-coord-a 14・own-coord-b 16）にし、struct の頭を `<S, C>` に直した。
- 変えた試験: field の直参照を owner の method に置き換えた（assertion は不変）。`structure_guard.rs` の `OWNER_STRUCTS` の名を
  `ExecutionCoordination<S, C>` に直した。cex_ の試験は変えていない。

## 証拠（Run 1、HEAD は未提出）

commit（件名）: 「phase 1: move task and work unit waits, process, channel and tick state to ExecutionCoordination」、
「phase 1: own-coord-b の途中経過を書き、Dispatcher の method と work_units.rs の行を base 以下に戻す」。

- `cargo nextest run -p task-dispatch -E 'test(structure_guard) | test(execution_coordination) | test(cex_) | test(phase_effect_ab)'`: 47 passed。
- それより前の版で、stale_priority・drain_hand_off・orphan_work_units・integration_ready_race・tree_approval・finished_plan・unit_task_deps・
  cron_jobs・write_set_gate・work_units を含む 156 件中 155 passed（落ちた 1 件は structure guard の doc の語。直した後に上の 47 件で pass）。
- `cargo clippy --workspace -- -D warnings`: exit 0（最初の commit で）。`cargo fmt --all -- --check`: exit 0。
- repo-health（base `$CELERIS_WU_BASE`）: fail_grade 1（上の未解決の 1 件だけ）。`Dispatcher` の method の数と `work_units.rs` の行数は base 以下に戻した。
- test-affected は未実施。
