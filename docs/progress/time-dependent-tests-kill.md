---
tasks: [01M3Y4AV5ZJNMV5W81EMTEV2CZ]
---
# 時間依存試験 (5): controller_kill_leaves_no_runtime_processes の決定化

- 日付: 2026-10-02
- 対象: `crates/task-worker/tests/browser_runtime_isolated.rs` の `controller_kill_leaves_no_runtime_processes`
- 方式: ADR-0125 §5。**(c) SIGSTOP stutter** で 2 つの競合点を固定、**(d)** 試験フックは `attack-test-hooks` feature の中だけ、終了判定は **(b)** 出来事待ち（上限 60 秒の保険）
- 前提: この worker sandbox で `unshare -U -r true` は exit 0（user namespace を使える）。plan_issue なし

## 原因

`runtime survived controller kill` には 2 つの候補があった。

1. **zombie の誤算（仮説）**: subreaper 配下で reap されない孤児を、`kill(pid,0)` や `/proc/<pid>` の有無で「生存」と数える。現版では `same_process_alive`（state `Z`/`X` と starttime 不一致を除く）で既に対処済みだった。
2. **実際の残存（c11ffd35 で修正済み）**: bwrap は `--info-fd` で init の PID を報告してから、子を `child_wait_fd` で解放する。PID の報告直後に controller が死ぬと、init が `PR_SET_PDEATHSIG` を設定する前に外側の bwrap が死ぬ。init は state `S`、PPid は subreaper のまま残る。c11ffd35 では `launch` が init の `do_wait`（設定済み）を待ってから戻るようにした。

ただし旧試験はどちらの競合点も scheduler 任せで通っていた。controller の死亡後は固定 10 秒の deadline で待っていた。負荷の高い host では「修正が効いているか」と「時間内に終わるか」を区別できなかった。

## 変更

- `browser_runtime.rs` に `test_hook`（`#[cfg(feature = "attack-test-hooks")]`）を追加した。env `CELERIS_TEST_RT_LAUNCH_HOOK_DIR` があるときだけ次のように動く。
  - info-fd の読み取り直後に init を SIGSTOP し、`launch-info` に PID を書く。
  - ready 待ちに入る前に `launch-waiting` を書き、試験が SIGCONT するまで待つ。10 秒の ready 上限は再開後から数える。保険の上限は 60 秒。
  - 本番 build（既定 feature）ではコンパイルされない。ready 判定・PDEATHSIG・kill/reap は変えていない。
- 試験は次の手順に変えた。
  1. `helper_reaper` を `PR_SET_CHILD_SUBREAPER` で起こす。その子が `helper_controller` で、controller の親が subreaper になる。
  2. `launch-info` を待つ。init は親死亡シグナルを設定する前で止まっている。
  3. `launch-waiting` と `pids` のどちらが先に出るかで、launch が ready を待ったかを判定する。待っていれば init を SIGCONT して `pids` を待つ。
  4. subreaper を SIGSTOP（stutter）してから controller を SIGKILL する。controller の終了（出来事）を待ってから init を SIGCONT する。
  5. runtime の 2 PID がどちらも「実行中の本人」でなくなるのを待つ（starttime 一致かつ `Z`/`X` 以外を生存とする。上限 60 秒は保険）。残れば `/proc/<pid>/stat` と PPid を出して失敗する。
  6. stutter が効いていることを assert する。止めた subreaper の下で bwrap が `Z` のままであること。
  7. subreaper を SIGCONT し、全孤児の reap による reaper の終了（出来事）を待つ。
  8. 失敗時も `Drop` で SIGCONT と SIGKILL を送り、試験の process を回収する。
- 意図は弱めていない。controller の死亡後に実行中の bwrap / init が残れば失敗する（assert の文言も同じ）。skip の扱いも変えていない（`CELERIS_ISOLATION_TESTS=skip` のときだけ `SKIPPED (not passed)`、bwrap / browser の欠如は失敗）。

## 修正前に落ち、修正後に通ること

host の load average は 29〜38（共用 host の通常負荷）。CPU を焼く負荷はかけていない。

| 条件 | コマンド | 結果 |
|---|---|---|
| 修正前の再現: c11ffd35 の ready 待ちだけを一時的に外す（commit しない） | `cargo test -p task-worker --test browser_runtime_isolated controller_kill_leaves_no_runtime_processes -- --exact` ×3 | 3/3 FAILED: `running runtime survived controller kill (launch waited for init: false)`、`(bwrap) S`、PPid = subreaper |
| zombie 誤算の再現: 試験の生存判定を一時的に `kill(pid,0)` へ替える（commit しない） | 同上 ×1 | FAILED: bwrap 2 PID が `Z`（PPid = 止めた subreaper）なのに生存と数えた |
| 修正後（現コード + 本変更） | 同上 ×5 | 5/5 ok（各 0.08s） |
| 試験一式 | `cargo test -p task-worker --test browser_runtime_isolated` | ok. 5 passed; 0 failed; 2 ignored |
| skip 規則 | `CELERIS_ISOLATION_TESTS=skip cargo test … controller_kill -- --nocapture` | `SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip` |
| lint | `cargo fmt --all -- --check` / `cargo clippy -p task-worker --all-targets -- -D warnings` / `cargo clippy -p task-worker --lib -- -D warnings` | いずれも exit 0 |

stutter は「対象 process を任意の長さ止める」ことと同じなので、負荷で init や subreaper が遅れる場合を最悪の形で含む。修正後の判定は出来事と `/proc` の状態だけで決まり、経過時間には依らない。

## 未解決・メモ

- ADR-0125 §5 にある残りの窓は未対処（本試験の範囲外、本番コードの課題）。`launch` の**途中**（info-fd の報告後・init の ready 前）で controller が SIGKILL されると、`launch` 自身の回収処理は走らない。このとき init が残り得る。本フックで「info 段で止めて controller を殺す」試験を作れば再現できるはずだが、直すには bwrap 側の起動順か外部の回収（`reap_recorded`）が要る。
- `launch` の ready 待ち 10 秒は本番の上限のまま。フック下では再開後から数えるので、試験の成否には影響しない。

## final review の zombie 証拠失敗への修正（fix-kill-2）

final review では controller と runtime の終了判定を通った後、`bwrap must be an unreaped zombie under the stopped subreaper` で bwrap の `/proc` が消えていた。reaper 停止中に bwrap が消えるのは、親から継承した `SIGCHLD=SIG_IGN` による kernel の auto-reap と整合する。従来の試験は SIGCHLD の disposition と mask を初期化せずに helper を起こしていた。また `SIGSTOP` を送った直後、停止状態の確認前に controller を殺していた。

修正前の試験バイナリを `/bin/sh -c 'trap "" CHLD; exec "$1" --exact controller_kill_leaves_no_runtime_processes --nocapture' sh <test-binary>` で直接起動すると exit 101。`helper_controller` の `NoChildPid` と親試験の `No child processes`（`try_wait`）を再現した。`trap` を cargo に直接かけると cargo 自身が rustc の終了を wait できず `ECHILD` になるため、先に `cargo test -p task-worker --test browser_runtime_isolated --no-run` でバイナリを作ってから直接起動した。

試験本体と `helper_reaper` の開始時に SIGCHLD を `SIG_DFL` に戻し、mask から外す。controller を殺す前に reaper の `/proc` state が `T`/`t` になるまで待つ。従来の zombie assert は維持し、bwrap の PPid がその停止中の reaper であることも確かめる。実行中の bwrap/init が残れば失敗する判定と、`CELERIS_ISOLATION_TESTS=skip` だけを許す規則は維持した。

この worker sandbox は `unshare -U -r true` が exit 1（`uid_map: Operation not permitted`）なので、実 runtime の通常起動・SIGCHLD 無視起動での反復 pass と final review の全コマンド exit 0 はここでは得られない。修正後の直接起動は、両条件とも `helper_controller` の `NoChildPid` により `launch-info` 前で失敗し、SIGCHLD 無視時の親 `ECHILD` は消えた。代わりに追加した `ignored_sigchld_still_keeps_unreaped_child_after_reset` は、`trap '' CHLD` の下で起こした helper が `/bin/true` を unreaped zombie として保持し、明示的に wait できることを 5/5 exit 0 で確認した。`cargo fmt --all -- --check` と `cargo clippy --workspace --all-targets -- -D warnings` も exit 0。

final review と同じ check コマンド列は、task-dispatch の 3 コマンドと `cargo build -p task-worker --bins` まで exit 0。その次の `browser_injection_wire` は real browser 試験の `unshare: Operation not permitted` で exit 101 となり、コマンド列の最後の `browser_runtime_isolated` には到達しなかった。これは上記の sandbox 制限と同じで、pass とは記録しない。

userns を使える host で `cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes` を通常起動と、事前ビルド済み試験バイナリの `trap '' CHLD` 起動でそれぞれ反復し、続けて final review の check コマンドを再実行する必要がある。

## 実環境での確認（最終コード、2026-10-02、ADR-0079 D7 real-env-2 人の回答）

人が `docs/PROGRESS.md`「時間依存試験の決定化」の「人が実行する手順」を、merge-main 完了後の最終 SHA `ab1914e629d9`（この fix-kill-2 節の SIGCHLD 継承修正 `e64043be` と main merge `a46b7423` を含む）で実行した。host `home-dev`、`unshare -U -r true` は exit 0、load average 14〜18（CPU を焼く負荷なし）。`ab1914e629d9` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカル、`CELERIS_USERNS_TESTS=1` で実行した。`git diff --stat a46b7423 ab1914e629d9 -- crates` は空（crates の tree は同一）。

- 通常起動 ×3: 3/3 `test result: ok`（各 0.08s、`helper_reaper` 0.10s）。`runtime survived`・`SKIPPED` の出力なし。
- `trap '' CHLD` 下（SIGCHLD 無視が継承される final review の条件）×3: 3/3 `test result: ok`。`bwrap must be an unreaped zombie`・`panicked` の出力なし。
- SIGSTOP stutter（`STUTTER_SCOPE=pid`）×3: 3/3 `test result: ok`。
- SIGCHLD 無視 + pid stutter の組み合わせ ×3: 3/3 `test result: ok`。

結果: 全件合格（4条件 × 3回 = 12回、すべて `test result: ok`）。未解決の失敗なし。本節が未解決としていた「userns を使える host での反復」はこれで満たされた。コードの変更はこの記録には含まれない。
