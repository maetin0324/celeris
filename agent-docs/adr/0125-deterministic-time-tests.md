# ADR-0125: 時間依存試験の決定的な同期

---
tasks: [01M3Y4FZD5HH800XATG3K5KQ28]
---

- 日付: 2026-10-02
- 状態: Accepted（方式の決定。実装は後続 WorkUnit）
- 対象: `task-dispatch` と `task-worker` の下記 5 試験

## 背景と共通規則

試験の正しさを「速い host で N 回の tick または数秒以内に終わること」に依存させない。状態の変化は保存された task / run / wait の状態、event、子 process の観測で判定する。待ちの上限は原則 30 秒以上の**異常時の保険**とし、上限に達したときは最後の状態・event・PID と `/proc` 情報を出す。時間そのものが仕様の検査は注入時計または制御した timeout 結果で進める。短い sleep は scheduler への譲りとして使えても、成立条件にはしない。

以下の方式を使う。(a) `tokio::time::pause` または注入した時計、(b) 固定 tick 数や短い deadline をやめ状態変化の通知・event を待つ、(c) 子 process の競合点を `SIGSTOP` / `SIGCONT` で固定する stutter、(d) 競合点に試験専用の遅延・同期フックを置く。状態を待つ側は通知の取りこぼしを避けるため、購読を先に作り、その後に現在状態も読む。手動 `tick()` が必要な dispatcher 試験では、状態を確認しながら tick を駆動し、tick 回数を失敗条件にしない。

## 1. `a_wait_parks_the_task_polls_and_resumes_as_a_continuation`

### 原因

`crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` の `tick_until(..., 200/400, ...)` は、各 tick の後に 20 ms 眠る。worker と偽 poller は別の task / OS thread で進むので、負荷でその完了が遅れると、論理的な wait 状態に到達する前に tick 数を使い切る。poll 間隔は `test_now` で 31 秒進めているが、poll 結果の受信と continuation の完了は実 scheduler に依存する。固定 5 tick による「まだ次の poll がない」確認も、非同期 poll が残っていると判定位置が曖昧になる。

### 方式

**(a) + (b)**。既存の `Dispatcher::test_now` を維持し、30 秒間隔や deadline の仕様は注入時計で検査する。`Blocked`、`ClusterJobWaitPolled`、poll 要求回数、`Satisfied`、`Done` を、それぞれ保存状態か event の到着を条件として待つ。待ちの実時間上限は 30 秒以上の保険とする。時計を進めない区間では、最初の poll が完了したことを先に確認してから tick を重ね、poll が 1 回だけで event 重複が無いことを確かめる。`tokio::time::pause` だけでは OS thread の poll 完了を進められないため、ここでは既存の注入時計を選ぶ。

### フック点

本体に追加するなら `dispatcher/cluster_job_wait.rs::poll_cluster_job_waits` の poller 起動・結果受信境界に、試験から `Notify` 等を受け取れる **`cfg(test)`** の通知フックを置く。既存の注入 poller と `test_now` はそのまま使う。製品 build ではフックをコンパイルしない。

待機中に slot と attempts を保持しないこと、同じ job 状態で event を重複記録しないこと、両 job が `F` になってから continuation が前置きを受けて `Done` になることを全て残す。

## 2. `every_cargo_path_uses_the_scratch_target_dir`

### 原因

`crates/task-dispatch/src/dispatcher/tests/build_cache.rs` の `run_until(..., 800, Done)` は最大約 16 秒の tick 予算で、並列 WU、checks、統合、reviewer を走らせる。レビューは `Reviewing` を経由するので、検査の完了前に 800 tick に達すると `Reviewing` のまま失敗する。`TargetDirAdapter` の 50 ms 遅延、worktree と shell 起動、review の実時間が混在し、tick 数は進捗の尺度にならない。

### 方式

**(b)**。WU run、各 WU check、統合 check、review check の記録を追い、最後に task の `Done` event / 状態を待つ。dispatcher は状態が変わるまで tick を駆動し、各段で観測した状態を診断に残す。上限は 30 秒以上の保険とし、`Reviewing` を成功扱いしない。最終状態だけでなく、`request.json` と記録した全 `CARGO_TARGET_DIR` が owner ごとの scratch target と一致する現在の検査を保持する。

### フック点

本体に足すなら WU 完了と review 完了の状態遷移・event append 後に **`cfg(test)`** の通知フックを置く。状態は store が正本であり、通知だけを根拠にしない。製品 build では無効。

## 3. `command_checks_are_re_executed_in_workspace`

### 原因

`crates/task-dispatch/src/review/tests.rs` は `LocalWorkspace` で `test -f`、`exit 7`、`sleep 30` を順に実行し、`review_task` に 3 秒 timeout を渡す。実際は timeout 時に 6 秒で 1 回再試行する。高負荷時に短い shell の起動まで遅れると、存在するファイルや期待終了コードまで timeout と誤判定する。一方 `sleep 30` を待つ試験は壁時計に 3 + 6 秒を消費する。

### 方式

**(d)**。workspace 内で再実行したこと（作業ディレクトリ、4 コマンドの順序、期待 exit 7、欠落ファイルの不合格）は実 `LocalWorkspace` で検査する。timeout と 2 倍再試行の枝は `Workspace::exec` の試験用実装が対象コマンドに明示的な timeout 結果を返し、呼び出し timeout が 3 秒→6 秒であることと最終 `timed out` を検査する。時計の実経過や `sleep 30` の完了速度を成立条件にしない。timeout の本番契約は別の process timeout 試験で検証し、この試験の timeout 枝を削らない。

### フック点

`review_task` は既に `&dyn Workspace` を受け取るので、本体への新フックは**不要**。試験側の `ScriptedWorkspace` / `Workspace::exec` 実装で結果を注入する（unit test の `cfg(test)` 範囲）。本番の timeout、再試行数、判定文言は変えない。

## 4. `real_broker_browser_injection_receipt_and_origin_guards`

### 原因

`crates/task-worker/tests/browser_injection_wire.rs` は `IsolatedRuntime::launch` の直後に `CdpController::agent_command("Target.createTarget", ...)` を送り、ここで `page target: SinkFailed` が起こる。runtime の準備完了は bwrap init と egress relay の準備を表すが、browser の CDP pipe が command に返答できる時点とは別である。`browser_cdp_sink.rs::read_response` は 5 秒の poll timeout で応答が来ないと `SinkFailed` にまとめる。現時点でこのエラーから断定できるのは **CDP 応答が得られなかったこと**までで、browser の起動遅延、早期終了、pipe の故障を診断で分ける必要がある。

### 方式

**(b) + (d)**。試験の `CdpController` で `Browser.getVersion` の有効な応答を CDP ready 条件として待ってから page target を作る。browser 終了も監視し、30 秒以上の保険期限では stderr・process 状態・最後の CDP 応答を報告する。試験用の CDP 応答待ちを少なくとも 30 秒に設定する。`Target.createTarget` の `SinkFailed` を成功や skip にせず、ready 後に発生すれば実障害として失敗させる。fixture page と iframe は現行の URL / frame の一致を待ち、receipt に秘密や selector が無いこと、origin 不一致・cross-origin iframe・auth section 外を拒否することを保つ。

### フック点

`browser_cdp_sink::CdpController::read_response` の 5 秒定数を、試験時だけ 30 秒以上の上限へ差し替えられるフックを置く。`Browser.getVersion` の応答を待つ側が CDP ready を判定する。統合試験ではライブラリの `cfg(test)` が有効にならないため、既存の `attack-test-hooks` と同じく**明示的な cargo feature** でコンパイルする。`cfg(feature = "attack-test-hooks")` の範囲でのみ試験用待機上限を設定でき、既定 feature と本番 binary は固定 5 秒のままにする。env だけでフックを有効化する方式は本番で誤起動できるため採らない。

この試験の現在の前提条件欠如は `tool()` / `browser()` で失敗する。`unshare` / browser / bwrap などが無い場合の挙動を skip に変更しない。

## 5. `controller_kill_leaves_no_runtime_processes`

### 原因

`crates/task-worker/tests/browser_runtime_isolated.rs` は helper controller を起こして 2 PID を受け取り、`SIGKILL` 後に sandbox process の消滅を待つ。旧式の `kill(pid, 0)` や `/proc/<pid>` 存在のみを `alive()` とすると、subreaper の配下で親にまだ reap されない zombie も生存と誤算する。**この仮説は現在のコードでは既に対処済み**: `process_liveness_counts_running_but_not_unreaped_zombie` は `/proc/<pid>/stat` の state が `Z` であることを確かめ、`same_process_alive` は state `Z` / `X` と starttime 不一致を生存から除く。対象試験も PID と starttime を保存し同関数を使う。したがって将来 `runtime survived controller kill` が再現したときは、診断に出す `/proc/<pid>/stat` の state と PPid を確認し、`Z` だけなら試験の誤判定、`R` / `S` / `D` 等で本人の starttime が一致すれば実際の生存と分ける。現版の同メッセージだけで zombie 原因とは断定しない。

もう一つの競合は、bwrap の info-fd 報告から init が `PR_SET_PDEATHSIG` を設定するまでの窓で controller が死ぬこと。`browser_runtime::launch` は現状 `/proc/<init>/wchan == do_wait` を待ってから runtime を返すが、試験はこの境界を scheduler 任せで通る。

### 方式

**(c) + (d)、終了判定は (b)**。info-fd 報告後・init の PDEATHSIG 準備前、および `launch` が準備完了を返した後の順序を試験フックで露出し、helper / 対象子を `SIGSTOP` で止めた stutter を作り、`SIGCONT` と controller の `SIGKILL` を制御して境界を再現する。少なくとも通常経路では 2 PID の starttime を記録し、controller 終了を event として待った後、両 PID が消えたか zombie / PID 再利用になったことを待つ。30 秒以上の保険上限で `/proc/<pid>/stat`、PPid、starttime を出す。`SIGCONT` は失敗時にも必ず送り、残る試験子を回収する。生きた同一 process を zombie として見逃さない。

### フック点

`browser_runtime::IsolatedRuntime::launch` の bwrap info-fd 読み取り直後と init 準備確認直後に同期点を置く。これは統合試験から使うため、既存の **`attack-test-hooks` cargo feature** でのみコンパイルする。フックは PID と段階を通知・待機させるだけで、本番の ready 判定、PDEATHSIG、kill / reap の意味を変更しない。env は feature が入った試験 binary のフック引数に限り、本番で env 単独では有効にならない。

`CELERIS_ISOLATION_TESTS=skip` を明示した場合のみ stderr に `SKIPPED (not passed)` を出す現在の規則を維持する。bwrap / browser 等の欠如を暗黙 skip にしない。

## 実装時の確認

5 件とも目的の拒否・成功判定は緩めない。特に (1) の待機中 lease 解放と continuation、(2) の全 cargo 経路の owner 別 scratch、(3) の timeout 再試行、(4) の秘密の非露出と origin / iframe 拒否、(5) の controller 死亡後に実行中の本人が残らないことを保持する。時間上限を広げるだけでは修正と見なさず、状態・event または制御した競合点を主判定にする。
