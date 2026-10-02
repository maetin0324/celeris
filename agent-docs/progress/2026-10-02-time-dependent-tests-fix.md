---
title: 時間依存試験の決定化
tasks: [01M3Y4AV5ZJNMV5W81EMTEV2CZ]
status: done
updated: 2026-10-02
---
# 時間依存試験の決定化

> 旧 `docs/PROGRESS.md` の節を ADR-0128 D6 に従い land-verify（task 01M3YBGM64RYPEY9NZANF79A0M）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

WorkUnit ごとの記録は [dispatch](2026-10-02-time-dependent-tests-fix/dispatch.md)・[injection](2026-10-02-time-dependent-tests-fix/injection.md)・[kill](2026-10-02-time-dependent-tests-fix/kill.md)、調査一覧は [2026-10-02-time-dependent-tests.md](2026-10-02-time-dependent-tests.md)。

## 時間依存試験の決定化（完了日 2026-10-02、[ADR-0125](../adr/0125-deterministic-time-tests.md)）

- 方式: (1) `a_wait_parks_the_task_polls_and_resumes_as_a_continuation` は注入時計 + 状態/event 待ち、(2) `every_cargo_path_uses_the_scratch_target_dir` は状態/event 待ち、(3) `command_checks_are_re_executed_in_workspace` は試験 workspace が timeout 結果を注入、(4) `real_broker_browser_injection_receipt_and_origin_guards` は CDP ready 待ち + 試験専用遅延、(5) `controller_kill_leaves_no_runtime_processes` は SIGSTOP/SIGCONT stutter + 同期フック + process 終了待ち、(6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event` は `target/` 消滅と `WorkspacePruned` event 到着を 1 つの待ちループで待つ（状態/event 待ち）。原因、意図を保つ条件、詳細な方式は [ADR-0125](../adr/0125-deterministic-time-tests.md) と[調査一覧](2026-10-02-time-dependent-tests.md)を参照。
- (1) `a_wait_parks_the_task_polls_and_resumes_as_a_continuation` — 原因: 旧 `tick_until` が 200 / 400 tick の上限と各 tick 後の 20 ms sleep で、別 OS thread の偽 poller と worker の完了速度を仮定していた。poll 間隔は注入時計だが、結果の受信は実 scheduler に依存し、負荷で完了が遅れると wait 状態到達前に tick 数を尽きさせ「200 ticks で条件に届かない」にしていた。方式: (a)+(b) で `test_now` 注入時計を維持しつつ `Blocked`・`ClusterJobWaitPolled`・`Satisfied`・`Done` を保存状態・event の到着で待ち、tick 回数は失敗条件から外して壁時計 60 秒を保険にする。poll が 1 回だけ・event 重複なし・continuation が job 最終状態を前置きにして `Done` になる検査は元のまま残す。
- (2) `every_cargo_path_uses_the_scratch_target_dir` — 原因: 旧 `run_until(..., 800, Done)` が最大約 16 秒の tick 予算で並列 WU・checks・統合・reviewer を走らせ、レビューが `Reviewing` を経由する間に 800 tick に達すると `Reviewing` のまま失敗していた。tick 数は `TargetDirAdapter` の 50 ms 遅延や worktree・shell 起動の実時間を測れず、負荷で進捗と一致しなかった。方式: (b) で各 WU run・check・統合 check・review check の記録を追って最終 task の `Done` 状態を待ち、`run_until_state` の壁時計 60 秒を保険にする。`Reviewing` を成功扱いにせず、全 `CARGO_TARGET_DIR` が owner ごとの scratch target と一致する検査を保持する。
- (3) `command_checks_are_re_executed_in_workspace` — 原因: 実 `LocalWorkspace` で `test -f`・`exit 7`・`sleep 30` を 3 秒 timeout（timeout 時 6 秒で 1 回再試行）で順に実行していたため、負荷で短い shell の起動自体が遅れると存在するファイルや期待終了コードのケースまで `timed out` と誤判定する危険があった。方式: (d) で通常の pass/fail 判定は負荷で誤判定されない長さの timeout を保険として実 `LocalWorkspace` で検査し、timeout / 再試行の枝は `ScriptedWorkspace` が `sleep 30` に明示的な timeout 結果を注入して決定的にする。3 秒→6 秒の 1 回再試行・最終 `timed out` の文言・期待 exit 7・欠落ファイルの不合格は保持する。
- (4) `real_broker_browser_injection_receipt_and_origin_guards` — 原因: `IsolatedRuntime::launch` の直後に `Target.createTarget` を送っていたが、runtime ready（bwrap init と egress relay の準備）と browser の CDP pipe が応答できる時点は別であり、browser 起動が遅れると CDP 応答の 5 秒 poll timeout が `page target: SinkFailed` にまとまっていた。方式: (b)+(d) で `Browser.getVersion` の有効応答を CDP ready 条件として 60 秒の保険期限で待ってから page target を作り、応答ごとの timeout は試験用 feature のフックで 30 秒以上に差し替える。ready 後に `SinkFailed` が起きれば実障害として失敗を維持し、receipt に秘密が無いことと origin 不一致・cross-origin iframe・auth section 外の拒否を保持する。
- (5) `controller_kill_leaves_no_runtime_processes` — 原因: controller `SIGKILL` 後の PID 生存判定が `/proc/<pid>` 存在のみだと subreaper 配下で未 reap の zombie を生存と誤算するほか、bwrap の info-fd 報告から init が `PR_SET_PDEATHSIG` を設定するまでの窓で controller が死ぬ競合があり、scheduler 任せの順序では境界が再現できなかった。方式: (c)+(d)、終了判定は (b) で、試験フックで info-fd 報告直後に init を `SIGSTOP` し、subreaper も `SIGSTOP` したまま controller を `SIGKILL` してから `SIGCONT` する stutter で順序を固定する。生存判定は `Z` / `X` と starttime 不一致を除外し、同一 process の消滅・zombie 化を 60 秒の保険で待ち、継承した `SIGCHLD=SIG_IGN` による auto-reap 防ぐため試験本体・subreaper で SIGCHLD の既定値と mask を復元する。
- (6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event` — 原因: `prune_one_workspace` は削除を別スレッドに逃がしてから `WorkspacePruned` event を追記するが、旧試験は `tick()` 後に `target/` の消滅だけを `for _ in 0..100`（20 ms sleep、約 2 秒）の固定回数で待って直後に event を読んでいた。負荷では unlink が終わっても event の sqlite 書き込みが未完了の窓があり、`cleanup_and_disk.rs:97` の assert が先に落ちていた。方式: (b) で固定回数ループを削除し、`target/` の消滅と `store.events_for` に期待の `WorkspacePruned { removed: ["repos/benchfs/target"] }` が現れることの両方を `wait_for_prune` の 1 つの待ちループで確認する。60 秒の `STATE_WAIT_GUARD` は超えたら現在の `events` と `target_dir.exists()` を出して `panic!` する保険で、`target/` 消滅・repo dir 残存・`removed` 中身の assert は元のまま保持する。
- 2026-10-02 単独再実行: `cargo test -p task-dispatch --lib cluster_job_wait` 3回、各 exit 0 / 5 passed。`cargo test -p task-dispatch --lib every_cargo_path_uses_the_scratch_target_dir` 3回、各 exit 0 / 1 passed。`cargo test -p task-dispatch --lib command_checks_are_re_executed_in_workspace` 3回、各 exit 0 / 1 passed。
- browser 試験: `unshare -U -r true` は exit 1（uid_map の `Operation not permitted`）。`cargo test -p task-worker --test browser_injection_wire` を3回実行、各 exit 101（各回 inner + delayed scripted の2 passed、real browser の `real_broker_browser_injection_receipt_and_origin_guards` は unshare の `Operation not permitted` で失敗）。`cargo test -p task-worker --test browser_runtime_isolated controller_kill_leaves_no_runtime_processes -- --exact` は exit 101（helper 起動時 `NoChildPid`、launch-info 前に終了し stutter 本体に未到達）。SIGSTOP stutter 条件での今回の実行も userns 不可のため未実施。skip 扱い・成功扱いにはしていない。過去の [kill 再実行記録](2026-10-02-time-dependent-tests-fix/kill.md)には userns が許可された環境での stutter 5/5 pass がある。
- (5) final review の bwrap zombie 証拠失敗は、継承した `SIGCHLD=SIG_IGN` による auto-reap と整合する。試験本体・subreaper で SIGCHLD の既定値と mask を復元し、reaper の停止と zombie の PPid も検証するよう修正した。[再現・修正記録](2026-10-02-time-dependent-tests-fix/kill.md)。
- `trap '' CHLD` 下の修正前バイナリは exit 101（親の `ECHILD`）。修正後の独立した zombie/reap 回帰試験、fmt、workspace clippy は exit 0。final review のコマンド列は dispatch 3 件と worker build まで通り、browser injection が userns 不許可で exit 101。実 runtime の反復は未確認。
- 統合検査: `cargo test --workspace` は exit 101、停止した最初の target `celeris --test instance_handoff` は 8 件中 3 passed / 5 failed（worker DB guard の user namespace `Operation not permitted` が3件、handoff/standby 状態待ち失敗が2件: `a_newer_release_takes_over_while_the_old_one_finishes_its_run`、`a_stale_heartbeat_promotes_the_standby`）。残り3失敗名は `normal_mode_does_not_inject_the_smoke_builtins`、`verify_mode_never_dispatches_and_never_touches_daemon_instances`、`starting_the_same_release_twice_exits_three`。cargo は後続 target を実行せず workspace 全体の件数は未確定。`cargo clippy --workspace -- -D warnings` は exit 0（警告なし）。`crates/` に差分なし。
- 未修正の同種試験: ファイル・試験名・待機の形は[調査一覧「その他の一覧」](2026-10-02-time-dependent-tests.md#その他の一覧)を参照。cluster wait の human/cancel/unknown-cluster/leaf wait、build cache の parallel target-dir、orphan takeover、tree approval/gate/replan、work-unit、browser startup reap、task-api stream / browser H3、credentiald injection IPC、process-group kill、browser egress、scratch-cache wait などを含む。
- 未解決事項: user namespace を許可する環境で実 browser injection と controller kill を各3回再実行し、controller kill は今回未実施の SIGSTOP stutter 条件でも再確認する。workspace test は userns 制限下で失敗し、別途2件の handoff/standby 状態待ちも発生したため、許可環境で再実行して全 workspace 件数を記録する。この記録 unit（verify-record）自体はコード変更なし。ブランチ全体では `crates/task-dispatch/src/dispatcher/tests/cleanup_and_disk.rs`（(6) の修正、後述）を含め `crates/` の試験を変更している。

### (6) `tick_prunes_the_oldest_terminal_workspace_and_records_an_event`（完了日 2026-10-02）

- 原因・方式: [`agent-docs/progress/2026-10-02-time-dependent-tests.md`](2026-10-02-time-dependent-tests.md#tick_prunes_the_oldest_terminal_workspace_and_records_an_event) の該当節を参照。`for _ in 0..100` の固定回数ループ（約2秒）を削除し、`target/` の消滅と `store.events_for` の期待 `WorkspacePruned { removed: ["repos/benchfs/target"] }` 到着を 1 つの待ちループ（`wait_for_prune`）で待つ。保険の [`STATE_WAIT_GUARD`]（60 秒）を超えたら現在の `events` と `target_dir.exists()` を出して `panic!` する。`target/` 消滅・repo dir 残存・`removed` の中身の assert は元のまま。同 file の `workspace_prune_after_secs_zero_disables_pruning` の 50ms sleep は直さない（`workspace_prune_after_secs == 0` は削除スレッドを立てずに即 return するため、待っても届かない非同期処理が無い）。
- 検証: `cargo test -p task-dispatch --lib cleanup_and_disk` を3回単独実行、各 exit 0 / 3 passed。`tick_prunes_the_oldest_terminal_workspace_and_records_an_event` 単体を SIGSTOP 2ms / SIGCONT 1ms の stutter 下で3回実行、3/3 `test result: ok`（stutter 無しの 0.03s に対し 0.07〜0.10s、崩れず通過）。`cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0。本体（`crates/task-dispatch/src/dispatcher/housekeeping.rs` の `prune_one_workspace` 等、非試験コード）は変更していない。

### 人が実行する手順（userns が使える host で、最終コード向け）

- 対象 SHA: main（`448891a2`）を取り込んだ merge commit `a46b7423`。この leaf（merge-main）の HEAD と、統合後の task branch の HEAD は crates/ の tree がこれと同じになる。確かめるコマンド: `git diff --stat a46b7423 <使う SHA> -- crates` の出力が空であること。この merge では `crates/task-worker/src/browser_runtime.rs` の init 待ちで衝突が出た。main の `NoChildPid(rt.failed_stderr())` を残し、このブランチの `test_hook::stop_init_after_info(rt.inner_pid)` は inner_pid が決まった直後に置いた。
- 前回の人の実環境確認は `57576a5a`（SIGCHLD 継承の修正 `e64043be` と main merge の前）。最終コードでは人の手ではまだ確かめていない。
- 前提: `unshare -U -r true` が exit 0。`CARGO_TARGET_DIR` はローカルを使う。CPU を焼く負荷はかけない。

1. (4) browser injection を単独で 3 回実行する:
   ```
   for i in 1 2 3; do cargo test -p task-worker --test browser_injection_wire -- --exact real_broker_browser_injection_receipt_and_origin_guards; echo "exit=$?"; done
   ```
2. (5) controller kill を単独で 3 回実行する。試験は自分で subreaper と init を SIGSTOP し、controller の SIGKILL 後に SIGCONT する（[kill 記録](2026-10-02-time-dependent-tests-fix/kill.md)の表、「修正後」行の stutter 条件）:
   ```
   for i in 1 2 3; do cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes; echo "exit=$?"; done
   ```
3. (5) を SIGCHLD 無視が継承される状態で 3 回実行する（final review で bwrap zombie の assert が落ちた条件）:
   ```
   for i in 1 2 3; do sh -c "trap '' CHLD; exec cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes"; echo "exit=$?"; done
   ```
4. 外からの SIGSTOP stutter（停止 2ms・再開 1ms、sleep だけで CPU は焼かない）の下で (4)(5) を各 3 回、(5) は SIGCHLD 無視の下でも 3 回実行する。(5) は `STUTTER_SCOPE=pid` で試験 process だけを止める。process group 全体を止めると外からの SIGCONT が、試験が止めた subreaper を起こしてしまい、試験の前提が壊れる。この worker で group 指定にすると bwrap の zombie assert が 6/6 落ちたが、これは試験側の不具合ではない。
   ```
   cargo test -p task-worker --test browser_injection_wire --no-run && cargo test -p task-worker --test browser_runtime_isolated --no-run
   D=$CARGO_TARGET_DIR/debug   # 未設定なら target/debug
   B4=$(ls -t $D/deps/browser_injection_wire-* | grep -v '\.d$' | head -1)
   B5=$(ls -t $D/deps/browser_runtime_isolated-* | grep -v '\.d$' | head -1)
   cat > /tmp/stutter.sh <<'EOF'
   #!/bin/sh
   # usage: [STUTTER_SCOPE=pid|group] [BIN_DIR=<target>/debug] stutter.sh <test-binary> <test-name>
   d=${BIN_DIR:-/nonexistent}
   # dash は名前に '-' を含む env を落とすので、(4) が実行時に読む CARGO_BIN_EXE_* は env(1) で渡す
   setsid env "CARGO_BIN_EXE_celeris-browser-sandboxd=$d/celeris-browser-sandboxd" \
     "CARGO_BIN_EXE_celeris-browser-egress=$d/celeris-browser-egress" \
     "$1" --exact "$2" --test-threads=1 &
   pid=$!
   t=$pid; [ "${STUTTER_SCOPE:-pid}" = group ] && t=-$pid
   while kill -0 "$pid" 2>/dev/null; do
     kill -STOP -- "$t" 2>/dev/null; sleep 0.002
     kill -CONT -- "$t" 2>/dev/null; sleep 0.001
   done
   wait "$pid"
   EOF
   chmod +x /tmp/stutter.sh
   for i in 1 2 3; do BIN_DIR=$D STUTTER_SCOPE=group /tmp/stutter.sh $B4 real_broker_browser_injection_receipt_and_origin_guards; echo "exit=$?"; done
   for i in 1 2 3; do STUTTER_SCOPE=pid /tmp/stutter.sh $B5 controller_kill_leaves_no_runtime_processes; echo "exit=$?"; done
   for i in 1 2 3; do sh -c "trap '' CHLD; STUTTER_SCOPE=pid exec /tmp/stutter.sh $B5 controller_kill_leaves_no_runtime_processes"; echo "exit=$?"; done
   ```
5. 合格の見分け方:
   - 各回 `exit=0` で、末尾が `test result: ok. 1 passed`（`FAILED` ではない）。
   - (5) の出力に `runtime survived`、`bwrap must be an unreaped zombie`、`panicked` が無い。
   - (4) の出力に `SinkFailed` と `panicked` が無い。
   - `CELERIS_ISOLATION_TESTS=skip` を設定していないのに `SKIPPED` と出たら、環境の不備（bwrap や browser が無い）で、合格ではない。
- 結果は `agent-docs/progress/2026-10-02-time-dependent-tests-fix/injection.md` と `agent-docs/progress/2026-10-02-time-dependent-tests-fix/kill.md` に追記する。
- 2026-10-02 の merge-main worker で上の 1〜4 を `a46b7423` で実行した（この sandbox では `unshare -Ur true` が exit 0、load average 約 31）。結果:
  - 手順 1〜3: 各 3/3 `test result: ok`。`SinkFailed`、`runtime survived`、`SKIPPED` は出なかった。
  - 手順 4: (4) は pid 指定・group 指定とも 3/3 ok。(5) は pid 指定で 3/3 ok、SIGCHLD 無視下で 3/3 ok。
  - 手順 4 の限界: 外からの stutter で実行時間はほとんど変わらなかった（(4) 0.43〜0.48s、(5) 0.08〜0.10s）。止められるのは試験 process とその process group だけで、自分で session を作る runtime の中までは届かないと見られる。競合点への負荷の再現は、試験の中にある stutter（(5)）と遅延（(4) `delayed_cdp_page_target_response`）が担う。
  - これは人の host での確認の代わりではない。

### 実環境での確認（2026-10-02、ADR-0079 D7 人の回答）

人が上記の手順を userns の使える host（host `home-dev`）で実行した。`unshare -U -r true` は exit 0。load average 18.5（CPU を焼く負荷はかけていない）。task branch `57576a5a` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカルを使用。

- (4) `cargo test -p task-worker --test browser_injection_wire -- --exact real_broker_browser_injection_receipt_and_origin_guards` を3回単独実行: 3/3 `test result: ok`（1 passed、各回 0.45〜0.49s）。`SinkFailed`・`SKIPPED` の出力なし。
- (5) `cargo test -p task-worker --test browser_runtime_isolated -- --exact controller_kill_leaves_no_runtime_processes` を3回単独実行: 3/3 `test result: ok`（1 passed、各回 0.08s、`helper_reaper` ok）。`runtime survived`・`SKIPPED` の出力なし。
- SIGSTOP stutter の修正前後比較（コードを一時的に外して再現させる手順）は、今回この人の実行では行っていない。[`agent-docs/progress/2026-10-02-time-dependent-tests-fix/kill.md`](2026-10-02-time-dependent-tests-fix/kill.md) にある、userns が許可された環境での過去の stutter 5/5 pass の記録を採用する。

未解決事項: 上記の (4)(5) は3回とも合格し、未解決の失敗はない。SIGSTOP stutter の修正前後比較（本来の手順3番目）は今回の人の実行では未実施（worker sandbox では userns が使えず自動実行できず、今回人が実行した際も改めてはやらず、過去の [kill 記録](2026-10-02-time-dependent-tests-fix/kill.md)の stutter 5/5 pass を根拠として採用したため）。

この記録は task branch `57576a5a`（SIGCHLD 継承の修正 `e64043be` と main merge の前）に対するもので、最終コードでの確認は下の「実環境での確認（最終コード）」に置き換わる。

### 実環境での確認（最終コード、2026-10-02、ADR-0079 D7 real-env-2 人の回答）

人が上記「人が実行する手順」の手順1〜4を、merge-main 完了後の最終 SHA `ab1914e629d9`（HEAD。`e64043be` の SIGCHLD 継承修正と main merge `a46b7423` を含む）で実行した。host `home-dev`、`unshare -U -r true` は exit 0、load average 14〜18（CPU を焼く負荷なし）。`ab1914e629d9` を `/var/tmp` の worktree に取り出し、`CARGO_TARGET_DIR` はローカル、`CELERIS_USERNS_TESTS=1` で実行した。`git diff --stat a46b7423 ab1914e629d9 -- crates` は空（crates の tree は `a46b7423` と同一）。

- 手順1 (4) browser injection 単独 ×3: 3/3 `test result: ok`（各 0.43〜0.52s）。
- 手順2 (5) controller kill 単独 ×3: 3/3 `test result: ok`（各 0.08s、`helper_reaper` 0.10s）。
- 手順3 (5) を `trap '' CHLD` 下で ×3: 3/3 `test result: ok`。
- 手順4 SIGSTOP stutter: (4) `STUTTER_SCOPE=group` ×3 で 3/3 ok（1.65s・1.73s・7.03s、stutter による遅延が効いている）。(5) `STUTTER_SCOPE=pid` ×3 で 3/3 ok。(5) SIGCHLD 無視 + pid stutter ×3 で 3/3 ok。
- 全回（合計18回）で `SinkFailed`・`runtime survived`・`bwrap must be an unreaped zombie`・`panicked`・`SKIPPED` のいずれの出力もなかった。

結果: 全件合格（推奨どおり）。未解決の失敗なし。最終コードでの (4)(5) の実環境確認・SIGSTOP stutter 条件（SIGCHLD 無視下を含む）は、ここで完了したものとして記録する。コードの変更はこの記録には含まれない。
