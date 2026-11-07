---
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
status: done
completed: 2026-11-07
---
# dispatch-ready-probe: dispatch_ready の段別計測

## 結論
- `dispatch_ready` の内側の 3 段（ready_tasks・dispatch_one・workspaces.worktree_base。gates は 38〜60 ms と小さいので外した）を `dispatch_ready.*` として `slow tick phases` の `steps_us` に出すようにした（挙動は変えない。[ADR](../../adr/2026-11-07-dispatch-ready-phase-timing.md)）。`tick_steps.rs` は変えていない（既存の `mark_with` を使った）。
- 本番 DB の写し（`20261107-182359-pre-66d9c1dae567.sqlite3`）の手動計測では、dispatch_ready の時間のほぼ全部が `dispatch_ready.workspaces.worktree_base`（`task_workspaces_for` の中の `worktree_base_for`。git の rev-parse で base を決める）だった。呼び出しは `dispatch_one` の中の `wu_dispatch_gate` → `parallel_mode` → `task_workspaces_for` で、ready の候補ごとに毎 tick 走る。他の段（ready_tasks 約 4 ms、gates 約 40 ms、events 読み込み 約 11 ms、assign 約 1 ms）は小さい。
- 写しでは定常 約 220〜320 ms、時々 0.7〜3.8 秒に跳ねた。本番の平均 1.9 秒・最大 6 秒と近い桁で、跳ねる原因は git のプロセスとファイル I/O の待ち（写しの計測中も同じ host の他の作業と競合する）と推定する。**未確認**: 本番での値。

## 計測（手動。`tick_prod_copy_steps`、12 tick、最初の 1 tick を除く、ms）
`TICK_PROD_DB=<写し> TICK_PROD_TICKS=12 cargo nextest run --run-ignored all -p celeris --test tick_prod_copy_steps --no-capture`

| 段 | 中央値 | 最大 |
|---|---|---|
| dispatch_ready | 398 | 3880 |
| dispatch_ready.dispatch_one | 約 330 | 3832 |
| dispatch_ready.workspaces.worktree_base | 304 | 3807 |
| dispatch_ready.ready_tasks | 4 | 6 |

上の値は最終の 5 段に絞る前の計測（段を内側まで分けた版、12 tick）。内側の分解では `wu_dispatch_gate` が dispatch_one の 99%、その中の `parallel_mode` が 90% 以上で、`plan_active`・`human_gate`・`units`・`children`・`settle`・`promote`・`reopen_stale` はどれも 1 ms 未満、events の読み込みは約 11 ms だった。source-size の基準で `dispatcher.rs`・`work_units.rs`・`dispatch_one` を太らせられないため、その分解の計測コードは入れていない。最終の 5 段の版での再測定は下の「再測定」。

## 提案
- `worktree_base_for`（git の rev-parse の呼び出し）の結果を tick の中、または tick をまたいで短く保持する。同じ repo・同じ task の `parallel_mode` を run を起こさない候補でも毎 tick 呼ぶ必要があるかを確かめる。これは本 WorkUnit の範囲外（計測だけ足した）。
- 本番では、promote 後の `slow tick phases` で `dispatch_ready.workspaces.worktree_base` が支配的かを確かめてから直す。

## 試験
- `crates/task-dispatch/src/dispatcher/tests/dispatch_ready_phases.rs`: tick の `step_us` に `dispatch_ready.*`（空の tick では ready_tasks・write_set・parallel_wus、ready の task があれば dispatch_one）が出て、同じ名前が 1 tick に 1 回だけ出ることを固定する。slow tick の log は同じ `step_us` をそのまま出す。

## 本番への移行
- config の変更は無い。
- promote 後に operator が見る値: journal の `slow tick phases` の `dispatch_ready`、`dispatch_ready.wu_gate`、`dispatch_ready.wu_gate.parallel_mode`、`dispatch_ready.workspaces.worktree_base` の中央値と最大（µs）。

## 未解決
- 本番の値は未確認（上は写しの値）。

## 再測定（5 段の版で測定。is_git_repo は 3 ms で、size 検査のため後で外した。12 tick、最初の 1 tick を除く、ms）
| 段 | 中央値 | 最大 |
|---|---|---|
| dispatch_ready | 273 | 3455 |
| dispatch_ready.dispatch_one | 231 | 3413 |
| dispatch_ready.workspaces.worktree_base | 212 | 3393 |
| dispatch_ready.ready_tasks | 4 | 4 |

## 検査（提出時の HEAD）
- `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`cargo nextest run -p task-dispatch dispatch_ready_phases` 1 件 pass。
- repo-health の size.responsibility-growth（dispatcher module の本体の行の合計が base 比で増えると fail）で一度落ちた。計測表を module の外の `crates/task-dispatch/src/phase_timing.rs` に移し、呼び出し点を drop で足す timer 1 行にして、module の行を増やさないようにした。`workspaces.is_git_repo` の段は外した。
- 最後の `bash scripts/dev/test-affected.sh` は exit 0（repo-health verdict=pass worsened=0、3500 件 pass）。途中の 1 回は task-dispatch 外の試験 1 件が `left: 3 right: 2` で落ちたが、どの試験かは未確認で、再実行では通った。

## 不安定な試験の切り分け（再試行）
対象: `dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`。前回は test-affected.sh（負荷下）で最終の task status が `Blocked`（期待は `Done`）になって落ちた。
- この変更（dispatch_ready の計測）との関係: 無い。試験は dispatch_ready の計測を見ず、base 側でも同じ構造。単独で 3 回流して 3 回 pass、task-dispatch 全体の nextest を 2 回流して 1464 件 pass（落ちず）。つまりこの変更で決定的に落ちるのではなく、負荷でだけ落ちる。
- 原因（コードの読みによる推定。落ちた現場の再現は未確認）: 落とした daemon（d1）の統合は `spawn_blocking` の git merge で、d1 を drop しても止まらない。試験は `sleep(200ms)` だけ待って d2 を作るので、負荷で旧い merge が終わっていないと、d2 の再統合と同じ worktree で git が競合し、統合が失敗して task が Blocked になる。待ち時間を 0 に縮めても手元では再現しなかった（競合は git の実行時機に依存する）。
- 直し（ADR-0125 の出来事待ち）: sleep をやめ、task branch に 2 件目の merge commit が載る（旧い統合の git 操作が終わる）まで、壁時計の保険（STATE_WAIT_GUARD、60 秒）つきで待つ。CPU 負荷や sleep の延長はしない。変更は `crates/task-dispatch/src/dispatcher/tests/work_units.rs` の当該試験だけ。直後に 3 回単独で流して 3 回 pass。
- 再試行後の提出時 HEAD で `bash scripts/dev/test-affected.sh` exit 0（3500 件 pass）、`cargo clippy --workspace -- -D warnings` と `cargo fmt --all -- --check` は exit 0。
