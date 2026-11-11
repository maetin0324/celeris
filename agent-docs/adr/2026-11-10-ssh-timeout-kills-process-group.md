---
tasks: [01M734DYF3Q45E7KJ91WJPSR69]
---
# ssh の timeout は子の process group と子孫ごと止める（ADR-0062 D2・ADR-0090 D2・ADR-0018 D5 への付記）

- 日付: 2026-11-10
- 状態: 採用

## 背景
2026-11-10 の release gate で、nextest は全試験を通して終わったのに、`scripts/dev/test-parallel.sh` が 45 分ほど doc test に進まなかった。
残っていたのは `sleep 3600` 1 つで、試験 `ssh::tests::command_probe_times_out_and_kills_a_hanging_ssh` の偽 ssh（`while true; do sleep 3600; done`）が fork したものだった。
`control_master_command_probe_blocking` は timeout で `Child::kill()` を呼ぶ。これは直接の子（偽 ssh の shell）だけを SIGKILL し、shell の子の `sleep` は孤児として 1 時間残る。
本番の ssh でも、`ProxyCommand`（`ssh -W` の踏み台など）や wrapper が起こした子は同じように残りうる。

`crates/task-worker/src/ssh.rs` には、timeout で直接の子だけを止める関数が 3 つある。
1. `control_master_command_probe_blocking`（ADR-0062 D2 の実通信 probe。tick から OS スレッドで呼ぶ）
2. `run_remote_command_blocking`（ADR-0090 D2 のクラスタ job の poll）
3. `run_command`（`SshWorkspace` の ssh・rsync。tokio の `timeout` が future を落とし、`kill_on_drop` が直接の子だけを撃つ）。rsync は ssh を子に持つ。

決定的 check の `LocalWorkspace::exec` は、2026-10-20 の ADR（`2026-10-20-integration-check-process-group-and-lease.md` の D1）で既に「独立の process group で起こし、時間切れでは group と子孫を SIGKILL する」に変わっている。

## 決定
- D1 3 つの関数は子を `process_group(0)` で起こす（子が自分を長とする新しい group に入る）。
  timeout では `process_group::kill_group_and_descendants`（`LocalWorkspace::exec` と同じ関数。先に ppid を辿って子孫を数え、group → 子孫の順に SIGKILL）を呼んでから子を `wait` で回収する。
  別の group・session へ移った子孫も、ppid の走査で拾う。
- D2 kill は子を回収する前に送る。回収前の子の pid と pgid は再利用されないので、無関係な process を撃たない（`process_group.rs` 冒頭の Phase 119 D2 の撤回と同じ規律）。
  - 同期の 2 関数は、`try_wait` が `None` を返した後（未回収）にだけ kill する。待ちと kill は ssh.rs の 1 つの helper（`wait_or_kill_group`）にまとめる。
  - `run_command` は、stdout・stderr を EOF まで読んでから `wait` する順にし、全体を 1 つの `timeout` で囲む。
    `timeout` が切れたときは、`wait` の future が `Ready` を返していない（子は未回収）ので、`child.id()` が `Some` の間だけ kill する（tokio は回収後に `None` を返す）。
- D3 timeout しないで子が終わった場合は、group に signal を送らない（回収後の pid は再利用されうる）。正常終了の後に残る子孫の掃除は、この ADR の範囲外とする。
- D4 旧い挙動（timeout で直接の子だけを `Child::kill()` する分岐）は消す。flag や互換の分岐は置かない。
- D5 試験の偽 ssh は、子（`sleep`）の pid も state に書く。試験は shell と子の両方が消えたことを確かめ、残った子を試験の終わりに必ず SIGKILL する（失敗した試験が孤児を残さない）。

## 範囲外（洗い出した結果は task の進捗ファイル `agent-docs/progress/2026-11-10-ssh-probe-orphan-test-parallel.md` に書く）
- ssh.rs 以外で、timeout のときに直接の子だけを止める箇所（`eval_trials.rs` の `run_eval_check`、`routing-eval` の `recheck.rs`、`auto_resolve/generated.rs` など）。別の task で同じ直し方にそろえることを提案する。
- `run_command` の future が timeout 以外で落とされた場合（run の中断）。tokio の `kill_on_drop` が直接の子だけを撃つ挙動は変えない。
- `LocalWorkspace::exec` の `GroupKillGuard` は、正常終了の後（子を回収した後）にも group を撃つ。この ADR では変えない。

## 帰結
- ssh・rsync・probe の timeout で、ProxyCommand・wrapper・試験の偽 ssh の子が孤児として残らない。
- 試験 `command_probe_times_out_and_kills_a_hanging_ssh` は、直す前の実装では子の `sleep` が残って落ちる。
