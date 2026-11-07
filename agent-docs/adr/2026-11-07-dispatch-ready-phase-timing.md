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
  - `dispatch_ready.ready_tasks`: ready task の取得と eligible の絞り込み（容量切れの早期 return の枠待ち記録も含む）
  - `dispatch_ready.write_set`: write-set の予約の掃除と、待たされた候補を先に並べる処理
  - `dispatch_ready.gates`: 候補ごとの依存統合待ち・子待ち・保守 task の判定
  - `dispatch_ready.dispatch_one`: `dispatch_one` 全体の合計（下の小段を含む）
  - `dispatch_ready.assign`: 担当の matching（`assign_if_needed`）
  - `dispatch_ready.gate`: Complexity Gate・WU の gate（`execution_gate_if_needed`・`wu_dispatch_gate`）
  - `dispatch_ready.task_dir`: 作業場所（worktree）の用意
  - `dispatch_ready.wu_workspace`: WU の worktree の用意
  - `dispatch_ready.route`: route 選択（`select_route_with`）
  - `dispatch_ready.acquire_lease`: lease 取得の store 書き込み
  - `dispatch_ready.append_worker_started`: `WorkerStarted` event の追記
  - `dispatch_ready.parallel_wus`: 並列 WU の 2 本目以降（この中の `dispatch_one` の小段も上の各段に加わる）
- D3. 小段は入れ子を含む（`dispatch_one` ⊇ `assign` など）ので、足し合わせて tick の時間にはならない。`dispatch_ready` 自体の値が全体で、`dispatch_ready.*` は内訳の観測用。
- D4. 本番写しの手動計測は `crates/celeris/tests/tick_prod_copy_steps.rs` を使い、結果は進捗 `agent-docs/progress/2026-11-07-root-done-tick-slow/dispatch-ready-probe.md` に書く。

## 本番への移行
config の変更は無い。promote 後は journal の `slow tick phases` の `dispatch_ready.*` の中央値・最大を見る。
