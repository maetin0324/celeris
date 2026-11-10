---
title: release gate の test-parallel が ssh probe の試験の孤児で戻らない不具合を直す
tasks: [01M734DYF3Q45E7KJ91WJPSR69]
status: done
updated: 2026-11-10
---
# release gate の test-parallel が ssh probe の試験の孤児で戻らない不具合を直す

完了日: 2026-11-10

## 結論
- 原因の側: `crates/task-worker/src/ssh.rs` の timeout で直接の子だけを止める 3 関数を、子の process group と子孫ごと止める形に直した（ADR `2026-11-10-ssh-timeout-kills-process-group`）。
- 受け手の側: `scripts/dev/test-parallel.sh` の `run_tee` は、cmd の側で tee への pipe の元の fd を閉じる。stdout を /dev/null にした子孫が残っても、`run_tee` は戻る（ADR `2026-11-06-test-runner-tmp-lifecycle` の付記 2026-11-10）。
- 全体の試験（`cargo nextest run --workspace`、doc test の一括、`test-parallel.sh` そのもの）は走らせていない。

## 変更
- `ADR: ssh の timeout は子の process group と子孫ごと止める` — `agent-docs/adr/2026-11-10-ssh-timeout-kills-process-group.md` を追加した（実装の前に書いた）。
- `ssh の timeout で子の process group と子孫ごと止める・run_tee は tee の pipe を cmd の stdout だけに持たせる` — 次を変えた。
  - `crates/task-worker/src/ssh.rs`
    - `control_master_command_probe_blocking`・`run_remote_command_blocking`: 子を `process_group(0)` で起こす。待ちと timeout を新しい helper `wait_or_kill_group` にまとめた。
      timeout では、子を回収する前に `process_group::kill_group_and_descendants`（`LocalWorkspace::exec` の時間切れと同じ関数）を呼び、その後で回収する。
    - `run_command`（`SshWorkspace` の ssh・rsync）: 子を `process_group(0)` で起こす。stdout・stderr を EOF まで読んでから `wait` し、全体を 1 つの `timeout` で囲む。
      timeout では、`child.id()` が `Some`（未回収）の間に group と子孫を SIGKILL してから回収する。旧い `command.output()` と `kill_on_drop` だけに頼る timeout は消した。
    - 旧い「timeout で `Child::kill()` を直接の子にだけ送る」分岐は 3 か所とも消した。flag・互換の分岐は置いていない。
  - `crates/task-worker/src/ssh/tests.rs`
    - `command_probe_times_out_and_kills_a_hanging_ssh` を書き直した。偽 ssh は子の `sleep 3600` を背景で起こし、shell と子の pid を state に書く。試験は両方が消えたことを確かめる。
    - 同じ形の試験を 2 つ足した: `remote_command_times_out_and_kills_the_ssh_process_group`・`run_command_times_out_and_kills_the_ssh_process_group`。
    - 試験の終わり（panic を含む）に、残った `sleep` を SIGKILL する guard（`KillLeftoverSleep`。comm が `sleep` のときだけ撃つ）を置いた。失敗した試験が 1 時間の孤児を残さない。
    - 守る不変条件: timeout の後に ssh の子孫が残らない。旧い試験は shell の pid だけを見ていたので、子の残りを検出できなかった。
  - `scripts/dev/test-parallel.sh`: `run_tee` の cmd を `ttl_run "$@" >&"$tfd" 2>&1 {tfd}>&-` で起こす。
  - `scripts/dev/tests/test-runner-tmp-lifecycle.sh`: 偽 cargo に `FAKE_ORPHAN`（stdin・stdout・stderr を /dev/null にした子孫を残して終わる）を足し、case (i) を足した。
    - 子孫は書き手の来ない fifo の `read -t 120` で止まる。CPU を焼く loop は使わない。
    - 試験は子孫を自分で kill する。途中で落ちても EXIT の cleanup が kill する（pid を `$PIDS` に書く）。漏れても 120 秒で終わる。
    - 確かめる点: runner が doc test へ進んで `test-parallel: ok` を出し、exit 0 で戻ること。子孫が pipe を 1 つも持たず、lock の fd を持つこと。子孫が居る間は dir が残り、子孫を止めた後の `ttl_sweep` で dir が消えること。
    - 守る不変条件: stdout を /dev/null にした子孫は `run_tee` を止めない。既存の case は子孫を残さないので、この不具合を検出できなかった。
  - `agent-docs/adr/2026-11-06-test-runner-tmp-lifecycle.md`: run_tee と lock の fd の付記（2026-11-10）を足した。

## 直す前の実装で試験が落ちたこと
- ssh: 試験だけを先に書き換え、`ssh.rs` が旧い状態で `cargo nextest run -p task-worker -E 'test(command_probe_) | test(times_out_and_kills_the_ssh)' --no-fail-fast` を 2 回走らせた。
  どちらも exit 非 0 だった。2 回目は `5 tests run: 2 passed, 3 failed`。落ちたのは `command_probe_times_out_and_kills_a_hanging_ssh`・`remote_command_times_out_and_kills_the_ssh_process_group`・`run_command_times_out_and_kills_the_ssh_process_group` の 3 件で、
  3 件とも `the child <pid> of the hanging ssh was left behind (only the direct child was killed)` だった（shell は消え、子の sleep が残った）。
  試験の guard が残った sleep を kill したので、この run の試験の孤児は残っていない。
- run_tee: `test-parallel.sh` を一時的に base（`git show HEAD:scripts/dev/test-parallel.sh`）に戻し、`CELERIS_RUNNER_TMP_TEST=1 bash scripts/dev/tests/test-runner-tmp-lifecycle.sh` を走らせた。結果は exit 1（`3 of 195 checks FAILED`、69 秒）。
  主な失敗は `test-parallel orphan: run_tee did not return while a descendant with stdout on /dev/null was alive` だった。runner は 60 秒の期限まで戻らず、試験が子孫を kill した後に戻った。
  その後で直した版に戻した（`git diff` で確認）。

## lock の fd（TTL_LOCK_FD）の扱い
- 子孫に継がせたままにした（閉じない）。
- 理由: ADR `2026-11-06-test-runner-tmp-lifecycle` の D2 は「runner は lock を開き、flock を取ったまま fd を子へ継がせる（CLOEXEC を付けない）。cargo・nextest・試験 process は、fd を閉じない限りこの lock を持ち続ける」と決めている。
  D3 の 1（lock を `flock -n` で取れなければ回収しない）は、孤児の子孫が使っている `TMPDIR` を消さないための保護で、この継承を前提にしている。
- `run_tee` が戻らなかった原因は、tee への pipe の fd だった。lock は通常の file の fd で、tee の EOF には関わらない。case (i) で、子孫が lock を持ったままでも `run_tee` が戻ることを確かめた。
- 2026-11-10 の本番の `sleep` が fd 10（lock）を持っていたのは、この設計どおりの挙動である。子孫が居る間、runner の dir は残る（終了時に `kept … (child still running or unknown)`）。次の起動の回収で消える。

## 同じ形の他の timeout の kill の洗い出し
`grep -rn "\.kill()" crates --include=*.rs`（試験を除く）と `grep -rln "kill_on_drop(true)"` で洗った。

直したもの（ssh.rs、この task）:

| 箇所 | 形 | 対応 |
|---|---|---|
| `task-worker/src/ssh.rs` `control_master_command_probe_blocking` | try_wait の loop、timeout で `child.kill()` | group と子孫を SIGKILL（`wait_or_kill_group`） |
| `task-worker/src/ssh.rs` `run_remote_command_blocking` | 同上 | 同上 |
| `task-worker/src/ssh.rs` `run_command` | tokio `timeout(output())` と `kill_on_drop` | group と子孫を SIGKILL してから回収 |

直していないもの（同じ形。子孫が残りうる順に並べた）:

| 箇所 | 起こすもの | 子孫が残る見込み |
|---|---|---|
| `task-dispatch/src/dispatcher/eval_trials.rs` `run_eval_check` | `sh -c <評価の check>`（cargo 等） | 高い。timeout で `sh` だけを kill する |
| `routing-eval/src/recheck.rs` `run_check` | `sh -c <check>` | 高い。同上 |
| `task-dispatch/src/auto_resolve/generated.rs` `run` | 生成物の再生成の program（pnpm など） | 中。program が子を起こせば残る |
| `campaign/src/dagbench/pbs/transport.rs` | PBS の transport の子 | 中。timeout で直接の子だけを kill |
| `campaign/src/cap_probe.rs` | toolchain の版の probe | 低い |
| `task-worker/src/container.rs` `probe_program`・`task-worker/src/browser_ledger.rs` `probe_agent_browser`・`eval_trials.rs` `run_version` | `--version` 系の probe | 低い |
| `celeris/src/control_path.rs` `run_ssh_g` | `ssh -G`（接続しない） | 低い |
| `celeris/src/releases.rs`（2 か所） | `git`（読み取り） | 低い。selfdeploy の高 risk path |
| `celeris/src/model_discovery.rs`・`task-worker/src/browser.rs`・`task-worker/src/browser_credential.rs` | tokio の `kill_on_drop` と `timeout`、process group なし | 未確認（起こす CLI の子の有無を見ていない） |

timeout の kill ではないので対象外にしたもの:
- `campaign/src/dagbench/process.rs` の `stop`、`dagbench/fault.rs` の StopControl: 握っている子だけを SIGKILL するのは障害注入の意図（コメントに明記）。
- `celeris/src/daemon/clusters.rs` の Drop、`celeris/src/cluster_admin.rs`: 明示の切断。
- `celeris-credentiald/src/injection_ipc.rs`: 試験のコード。
- `task-worker/src/browser_runtime.rs`: 独自の停止の手順を持つ。
- `task-ops/src/changes.rs`: 既に `process_group(0)` と group への SIGKILL。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
最終報告の本文に、受け入れ条件ごとのコマンドと結果を書く。ここには作業中の結果を書く。
- `cargo nextest run -p task-worker -E 'test(command_probe_) | test(times_out_and_kills_the_ssh) | test(remote_command_)'` → exit 0（6 passed）
- `cargo nextest run -p task-worker -E 'test(/^ssh::/) | test(/cluster/) | test(/remote/)'` → exit 0（61 passed）
- `CELERIS_RUNNER_TMP_TEST=1 bash scripts/dev/tests/test-runner-tmp-lifecycle.sh` → exit 0（`runner-tmp-lifecycle: ok（199 checks）`）

## 未解決
- この host に、旧い試験が残したと見られる `sleep 3600` が 5 つあった（この run の試験より前の 15:50〜16:21 に起動、親は pid 464、この run からは environ を読めない）。別の run のものなので触っていない。
  1 時間で自分から終わる。旧い試験が main から消えるまでは、他の task の test-affected が同じ孤児を増やしうる。確かめるには `pgrep -a -x sleep | grep 3600` を使う。
- `run_command` の future が timeout 以外で落とされた場合（run の中断）は、tokio の `kill_on_drop` が直接の子だけを撃つ挙動のまま（ADR の範囲外）。
- `run_remote_command_blocking` は、ssh が自分から終わった後、stdout・stderr の drain thread の join で、pipe を握る子孫の終わりを待つ。この経路は timeout の kill ではないので変えていない。

## 提案
- 上の「直していないもの」のうち、`eval_trials.rs` の `run_eval_check`、`routing-eval` の `recheck.rs` の `run_check`、`auto_resolve/generated.rs` の `run` を、同じ直し方にそろえる別 task を起こす。
  同じ直し方とは、`process_group(0)` で起こし、timeout では回収の前に `process_group::kill_group_and_descendants` を呼ぶことである。
  同期の try_wait の loop は ssh.rs の `wait_or_kill_group` と同じ形なので、`process_group.rs` へ移して共有するとよい。この task では ssh の owner の範囲に留めた。
- `run_command` の中断（future の drop）でも group を撃つ guard を置くか検討する。`LocalWorkspace::exec` の `GroupKillGuard` は回収の後にも撃つので、そのまま流用すると pid の再利用に当たりうる。回収前だけ撃つ guard（tokio の `Child::id()` が `Some` の間）を `process_group.rs` に置き、両方で使う案がある。

## 本番への移行
- config・DB・unit の変更は無い。release に入れば効く。
