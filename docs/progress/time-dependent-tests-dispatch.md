---
tasks: [01M3Y4AV5ZJNMV5W81EMTEV2CZ]
---

# task-dispatch の時間依存試験 3 件を決定的にした記録

方式の記号と方針は [ADR-0125](../adr/0125-deterministic-time-tests.md)、調査は [time-dependent-tests.md](time-dependent-tests.md) に従う。本体（製品コード）は変えていない。変えたのは試験と試験 helper だけ。

## 共通: 状態待ちの helper（`crates/task-dispatch/src/dispatcher/tests/mod.rs`）

- `STATE_WAIT_GUARD = 60s` と `run_until_state(d, done)` を追加した。store の状態・event（`done`）が真になるまで `tick()` を駆動し、tick の回数は失敗条件にしない。壁時計 60 秒を過ぎたときだけ `false`（壊れたときに止まる保険）。tick 間の 20 ms sleep は worker・検査の task への譲りで、成立条件ではない。
- 既存の `run_until(d, max_ticks, ..)` は他の試験が使うので残した（この WU の範囲外。一覧は time-dependent-tests.md）。

## (1) `dispatcher::tests::cluster_job_wait::a_wait_parks_the_task_polls_and_resumes_as_a_continuation`

- 変更前の失敗の説明: `tick_until(&mut d, 200, ..)` は「200 tick × (tick + 20 ms)」≒ 4〜5 秒の予算で、worker（tokio task）と偽 poller（OS スレッド）の完了を待っていた。load 20〜60 の host ではスレッドの起床が遅れ、条件（`Blocked`・`ClusterJobWaitPolled`・poll 回数・`Done`）に届く前に tick 数を使い切って `condition not reached after 200 ticks` で落ちた。さらに「次の poll を起こさない」の確認が固定 5 tick で、まだ走っている poll があると判定位置が曖昧だった。
- 方式 (a)+(b):
  - `tick_until(d, pred)` から tick 数の引数を消し、壁時計 `STATE_WAIT_GUARD`（60s）の保険で止める。失敗時は tick 数・`in_flight`・走っている poll の数を出す。
  - poll の間隔・上限は既存の注入時計（`advance` / `test_now`）で進める（変更なし）。
  - 固定 5 tick の否定確認は、`settle_polls`（走っている poll の結果を tick が拾い終える = `cluster_job_polls` が空になるまで待つ）＋ `tick_starts_no_poll`（1 回の tick の直後に `cluster_job_polls` が空のまま）に替えた。poll は tick の中で同期に `cluster_job_polls` へ載るので、スレッドの速さに依らず「poll_secs 前は起こさない」が決まる。
  - 2 回目の poll は「控えが 2 件 かつ その結果を拾い終えた」まで待ってから event 数（1 件のまま）を数える。
- 同じ helper を使う同ファイルの 4 試験（`a_timed_out_wait_asks_a_human_and_the_answer_resumes`、`cancelling_a_waiting_task_cancels_the_wait_without_qdel`、`a_wait_on_an_unknown_cluster_is_a_retryable_failure`、`a_leaf_unit_waits_alone_and_liveness_names_the_wait`）も同じ形に直した。cancel の固定 10 tick は `settle_polls` + `tick_starts_no_poll`、leaf の時刻ごとの固定 3 tick は「1 tick + `settle_polls`」に替えた。
- 意図の保持: attempts 据え置き・lease なし・slot を持たない・poll_secs のクランプ・一般回答の拒否・受信箱に出ない・poll スクリプト・状態不変の poll で event を出さない・satisfied → continuation の前置き・replay 差分 0 の assert はすべてそのまま。

## (2) `dispatcher::tests::build_cache::every_cargo_path_uses_the_scratch_target_dir`

- 変更前の失敗の説明: `run_until(&mut d, 800, Done)` は 800 tick（≒16 秒）の予算で、並列 WU の run（`TargetDirAdapter` の 50 ms 遅延）、WU の checks（実 sh）、統合 WU の検査、reviewer の checks を走らせていた。review は `Reviewing` を経由するので、高負荷で shell の起動が遅れると予算切れの時点で `Reviewing ≠ Done` になって落ちた。後半の Task 単位の run も `run_until_idle(&mut d, 60)`（60 tick）の後に `Done` を assert していた。
- 方式 (b): 両方とも `run_until_state(&mut d, || status == Done)` に替えた（保険 60s、失敗時は event 列を出す）。`Reviewing` は成功扱いしない。`request.json` の `cargo_target_dir`・WU lease の key/kind・全 `CARGO_TARGET_DIR` が owner ごとの scratch target であること・旧 `build_cache_dir/cargo` が作られないことの assert はそのまま。
- 同ファイルで同じ形の `parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done` も `run_until(800)` / `run_until(200)` / 10 秒 deadline のループを `run_until_state` に替えた（target の rename と `.deleting-*` の削除完了を出来事として待つ）。

## (3) `review::tests::command_checks_are_re_executed_in_workspace`

- 変更前の失敗の説明: 実 `LocalWorkspace` で `test -f present.txt` / `test -f absent.txt` / `exit 7` / `sleep 30` を **同じ短い timeout**（元は 300 ms、別 task で 3s に拡大）で走らせていた。高負荷で sh の起動が timeout を超えると、`test -f present.txt` のような正常コマンドまで `command timed out` になり pass/fail が崩れた（timeout を広げても負荷次第で再発し、`sleep 30` の待ちに 3s + 6s を払う）。
- 方式 (d)（ADR-0125 §3、本体へのフックは不要）:
  1. 実 `LocalWorkspace` で timeout しない 3 件を **120 秒の timeout**（壊れたときの保険）で実行し、`[true, false, true]`・欠落ファイル名が理由に出る・`criterion_idx`・timed out が一つも無いことを確かめる。
  2. 同じ 4 件を、`sleep 30` だけ timeout を返す既存の `ScriptedWorkspace` で `review_task` に通し、`[true, false, true, false]`・`timed out after 6s`・`sleep 30` の呼び出しが 3s → 6s の 2 回だけ（2 倍で 1 回の再試行）を確かめる。
  3. 実 process の timeout も別の呼び出しで残す: `sleep 30` だけを timeout 1s（再試行 2s）で実行し、`timed out after 2s` で不合格。負荷で遅れるほど timeout 側に倒れるだけなので、負荷で結果は変わらない。
- `plain_review` の引数を `&LocalWorkspace` から `&dyn Workspace` にした（既存呼び出しはそのまま型変換される）。

## 負荷に依らない根拠と確認

- 3 件とも成立条件は store の状態・event・明示した timeout 結果だけで、tick 数・短い deadline は残っていない（`git grep -nE 'tick_until\(&mut d, [0-9]|for _ in 0\.\.' crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` は 0 件）。壁時計は 60s / 120s の保険だけ。
- 確認したコマンド（2026-10-02、この worktree）:
  - `cargo test -p task-dispatch --lib` → exit 0、499 passed。
  - `cargo test -p task-dispatch --lib -- cluster_job_wait`（5 passed）、`every_cargo_path_uses_the_scratch_target_dir`・`parallel_work_units_get_their_own`・`command_checks_are_re_executed_in_workspace`（3 passed）。
  - SIGSTOP stutter: 試験バイナリを上の 8 試験で起動し、終わるまで `kill -STOP` 200 ms / `kill -CONT` 100 ms を繰り返した（11 周）→ exit 0、8 passed。CPU を焼く負荷は使っていない。
  - `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0。
- 変更前の失敗は CPU を焼く再現をせず（共用 host の規則）、上の各項の「予算 = tick 数 × 間隔」と非同期の完了の競合として説明した。
