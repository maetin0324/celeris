---
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
---
# ADR 2026-11-07: dispatch_ready の段別計測を slow tick log に出す

- 状態: 採用（計測の追加のみ。挙動は変えない）。
- 背景: 本番（release 66d9c1dae567）の slow tick 234 件で dispatch_ready は平均 1.9 秒、最大 6 秒だった。`cos.root_done_tick` と違い、`dispatch_ready` は 1 つの段として出るだけで、どの内側の処理が遅いか分からない。
- 関係: [ADR 2026-11-05 phase4 p4a baseline contract](2026-11-05-phase4-p4a-baseline-contract.md) D3（tick の段別計測）、`tick_cos_chat_launch` の `cos.*` 小段（`TickSteps::mark_with`）。

## 決定
- D1. `dispatch_ready` の内側を `dispatch_ready.<段>` の名前で `slow tick phases` の `steps_us` に出す。`TickSteps::mark_with`（既存）を使うので `tick_steps.rs` は変えない。値は µs で、tick 全体が `SLOW_TICK`（1 秒）以上のときだけ log に出る。判断には使わない。
- D2. 段は、Dispatcher の `dispatch_ready_parts`（tick ごとに空にする累積表）へ名前ごとに時間を足す。候補が複数あれば同じ名前の時間は合計になる。
  - `dispatch_ready.ready_tasks`: ready task の取得と eligible の絞り込み
  - `dispatch_ready.gates`: 候補ごとの依存統合待ち・子待ち・保守 task の判定
  - `dispatch_ready.dispatch_one`: `dispatch_one` 全体の合計（下の `workspaces.*` を含む）
  - `dispatch_ready.workspaces.is_git_repo`: 作業場所の解決（`task_workspaces_for`）での git repo の判定
  - `dispatch_ready.workspaces.worktree_base`: 同じく base の決定（`worktree_base_for`。git の rev-parse）
- D2a. 累積表は新しい `ready_phases.rs`（`ReadyPhases`）に置く。`dispatcher.rs`・`work_units.rs`・`dispatch_one` は source-size の基準（ADR-0083）で行を増やせないため、その中の段（assign・gate・route・lease・event、`wu_dispatch_gate` の内側）は測らない。最初の手動計測で `dispatch_one` の時間のほぼ全部が `worktree_base_for` だと分かったので、必要になったら呼び出し先の側に段を足す。
- D3. 小段は入れ子を含む（`dispatch_one` ⊇ `workspaces.*`）ので、足し合わせて tick の時間にはならない。`dispatch_ready` 自体の値が全体で、`dispatch_ready.*` は内訳の観測用。
- D4. 本番写しの手動計測は `crates/celeris/tests/tick_prod_copy_steps.rs` を使い、結果は進捗 `agent-docs/progress/2026-11-07-root-done-tick-slow/dispatch-ready-probe.md` に書く。

## 本番への移行
config の変更は無い。promote 後は journal の `slow tick phases` の `dispatch_ready.*` の中央値・最大を見る。
