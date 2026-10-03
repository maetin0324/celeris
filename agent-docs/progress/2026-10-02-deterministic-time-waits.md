---
title: e2e・結合試験の時間依存待ち（ADR-0125）
tasks: [01M3ZCXG7C34WZFJ46Q64XS8SZ, 01M3Z08A0T81ZQ60XVR62XJMPD]
status: done
updated: 2026-10-03
---
# e2e・結合試験の時間依存待ち（ADR-0125）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## e2e・結合試験の時間依存待ち（ADR-0125、task 01M3ZCXG7C34WZFJ46Q64XS8SZ）

- 2026-10-03（人の判断を反映・最新 main 取り込み）: main `f45f218e` を `git merge`。scratch-cache は main で撤去済みのため削除側、180 件待ちは main の deflake-lock `wait_for_progress` を採って重複を解消。launcher の host stutter 3/3（人の実行）と sccache_webdav_e2e の例外承認を[一覧](2026-10-02-deterministic-time-waits/inventory.md#人の判断による例外の確定と最新-main-の取り込み2026-10-03)に記録。
- 2026-10-03（最終 review 修正）: main `3527c8e3` 取り込み後の `notification_change_emits_a_reload_hint` に残った hello 2 秒・通知到着 7 秒を `EVENT_WAIT`（60 秒の保険）へ統一。通知到着・内容の主張は不変。修正後の stream 全 9 件を SIGSTOP stutter で各 3 回、全て exit 0。指定 build・api_scenarios・fmt・clippy の連続ゲートも exit 0。main は既に HEAD の祖先。詳細は[最終再検証](2026-10-02-deterministic-time-waits/inventory.md#main-取り込み後の通知試験の修正再検証2026-10-03)。
- launcher の現状: host は人が main `3527c8e3` / protocol v3 に更新済みで、必須モード 6 passed・real-session admission 5 行・EXIT 0 の通常実行 1 回をログで確認。run 内の必須モード stutter は権限付きでも SCM_CREDENTIALS UID が 65534 になる user namespace 制約で失敗し、host stutter 3 回は未確認。人の判断どおり sandbox の制約と host の証拠を区別する。手動 WebDAV 試験は対象外。host 設定変更・判定の緩和は行っていない。
- 2026-10-03（attempt 3）: review 指摘の `browser_runtime_isolated` の CDP 応答と `browser_runtime_supervisor` の SIGKILL 後消滅待ちを 30 → 60 秒へ。同型の CDP/socket/gate/fixture 終了待ちと埋め込み Python/shell も再走査し、出来事を主判定に 60 秒以上の保険へ揃えた。本番の既定値は変更せず、CDP は既存の試験用 feature を利用。クリック解放と fixture server 終了の失敗は無視せず検査する。
- attempt 3 の検証: main `aed80844` を取り込み、通常有効な変更対象 36 ファイルを SIGSTOP 300 ms / SIGCONT 後 100 ms の下で各 3 回、計 108 実行が exit 0（最上位 libtest 集計は 170 passed/回）。主要 4 件、追加 CDP 2 件、今回指摘の runtime/supervisor を含む。全ファイル別の件数・方式・機械ログの所在は[一覧](2026-10-02-deterministic-time-waits/inventory.md#第-3-走査後の全変更対象-stutter-記録)に追記。
- 検証の限界: launcher 必須モードは host protocol v1 の session binding 欠如で exit 101。通常モードでの 3 回は ptrace 拒否・起動・消滅が通過し、admission 表だけ `SKIP: (not passed)`。手動 `sccache_webdav_e2e` は server 操作と環境変数変更がこの run で禁止されているため ignored のまま（0 passed）。この 2 点を検証成功とは数えない。本番 host は変更していない。
- attempt 3 の指定ゲート: `cargo build --workspace --bins && cargo test -p e2e --test api_scenarios && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` は exit 0（api_scenarios 11 passed）。初回 clippy の未使用 Result 指摘は fixture 終了結果の検査を追加して解消し、releases_api の stutter も 3 回取り直した。
- attempt 3 の全体検証: `cargo test --workspace` は user namespace が使える権限付き環境で exit 0。ログは run の `workspace-final.log`。手動 ignored と launcher admission 表の環境制約は上記のとおり。
- 最終同期: main `0b8a2256` の文書のみの追加 2 行を取り込み、統合後に指定ゲートを再実行して exit 0（api_scenarios 11 passed）。全体試験・stutter 実行時からコードの変更はない。
- 2026-10-02: [時間依存待ち一覧](2026-10-02-deterministic-time-waits/inventory.md)に `tests/e2e/tests` と `crates/*/tests` の候補 45 ファイル、対象ごとの原因と方式を記録した。
- `daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`: 原因は worker の固定 6 秒と cooldown の短い 5 秒待ち。方式は release ファイルで worker を保持し、同時状態を観測後に解放する。保険超過時は `done` を返さない。
- `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`: 原因は 180 task を固定 120 秒で判定したこと。方式は全件 `Done` の観測、進捗停止 60 秒、総保険 600 秒。lock 不在と daemon 生存の主張は維持。
- `wait_api`: 原因は daemon 起動を `/health` の 20 秒待ちだけで判定したこと。方式は TCP listen を観測してから `/health` 200 を確認し、各 120 秒を保険とする。4 fixture に適用。
- `phase3_control_converges_rejects_competition_and_cancel_stops`: 原因は lease 失効を固定 2 秒 sleep で代用したこと。方式は `GET /control` の `paused` と holder 消滅を観測し、120 秒を保険とする。
- `instance_handoff`、`worker_run_signal`、`reap_finished_children`: 原因は 5〜30 秒の短い期限または 200 回ループ。方式は既存の状態・PID・zombie 観測を維持し、60 秒以上の保険へ変更した。
- `provider_admin_scenarios`: 原因は slow worker の固定 8 秒中に reload と dispatch が間に合う前提。方式は release ファイルで保持し、新 account への `WorkerStarted` を確認してから解放した。
- credentiald/browser の fixture socket、`releases_api` のログ、`unified_kill` の孫 PID・消滅: 原因は固定 40〜400 回の poll 予算。方式は各出来事の観測を主判定にして 60 秒の保険へ変更した。
- `crates/*/tests` のその他の fixture listen、process 終了、browser ready、WebDAV flush の短い deadline は状態を主判定に維持し、保険を 60 秒へ延長した。`browser_egress_process` の 8 秒など実時間の契約を検査する箇所はそのまま残した。
- 2026-10-03（review 差し戻し後の第 2 走査）: `Duration` の字面 200 ms〜60 秒の 95 行を全て判定した。`task-api` の `console.rs`（SSE hello 500 ms・`console.block` 3 秒）、`daemon_providers_config.rs`（hello・`daemon` 2 秒）、`stream.rs`（2 秒契約の 1 試験以外の到着待ち）、`standby.rs`（5 秒）は原因が短い固定期限の到着待ちで、方式は到着を主判定に 60 秒の保険 `EVENT_WAIT` へ。`browser_runtime_supervisor`・`browser_runtime_isolated`・`browser_shared_cdp`・`browser_egress_process`・`scratch-cache` の 1〜20 秒の消滅・出現・読み待ちも 60 秒の保険へ。不在の確認と仕様の時間検査は残した。SIGSTOP stutter で 6 試験 file を各 3/3。
- main（33aca5a3、続けて 41366893）を取り込み、main を HEAD の祖先にした。
- 既存修正は e2e-stable の f307d63b を `cherry-pick -x` で取り込んだ。deflake-lock ブランチはこの worktree から参照できず、同じ全件 Done 待ちは追加の進捗停止・総保険で対応した。後からの main 取り込み時には重複を確認する。
- 検証: `cargo build --workspace --bins` exit 0、`cargo test -p e2e --test api_scenarios` は 11 passed / exit 0。対象 4 試験は SIGSTOP/SIGCONT stutter 下で各 3/3 通過（詳細は一覧）。この run は user namespace probe が EPERM になるため、API fixture の worker DB guard のみ opt-out した。guard 専用試験は変更していない。

## api_scenarios の負荷 flaky 2 件を出来事待ちに（2026-10-02、task 01M3Z08A0T81ZQ60XVR62XJMPD 葉 e2e-stable）

`tests/e2e/tests/api_scenarios.rs` だけを変えた。主張（cooldown と in_flight の同時観測、`database is locked` が出ない、celeris が落ちない）はそのまま。

- `daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded`: 原因: slow の worker が `sleep 6` で終わるため、tick が遅いと cooldown を観測する前に slow が in_flight から消えていた。方式: worker は workspace の `release` file が現れるまで 0.1s 刻みで待ち、試験は cooldown・in_flight の確認後に `release` を置く。cooldown の待ちは 5s→30s。
- `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`: 原因: 全 task done を 120s の壁時計で待っていたため、負荷で tick が遅いと進んでいても失敗した。方式: 終わった task の数が 60s 増えないとき（または celeris が落ちたとき）だけ失敗する進捗待ちにした。
- 重複の可能性: 後者は sccache task 01M3YD2Z585N1YCBZK4AH8QXR0 の葉 deflake-lock も直す予定だった。実行時点で branch `celeris-wu/01M3YD2Z585N1YCBZK4AH8QXR0/deflake-lock` が無かったため同じ方式（終わった数が一定時間増えないときだけ失敗）で自前に直した。後で両方が main に入るときは衝突しうるので片方に揃える。
- `cargo test -p e2e --test api_scenarios` → exit 0（11 passed）。`cargo clippy -p e2e --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## ADR-0125: e2e・結合試験の時間依存待ち（task 01M3ZCXG7C34WZFJ46Q64XS8SZ）

`docs/testing/time-dependent-waits.md` に候補 45 ファイルと判定を記録した。`api_scenarios` の 4 件は状態・ファイル・listen 完了を待ち、60 秒以上の保険を置いた。SIGSTOP stutter は対象 4 試験を各 3 回実行して 12/12 pass。CDP の追加 2 件も `Browser.getVersion` の ready 応答を待つようにし、試験専用 CDP 応答上限を 30 秒、ready 上限を 60 秒にした。通常 sandbox は `unshare` を拒否したが、権限付き実行では両方 pass。追加 2 件の SIGSTOP stutter も各 3 回で 6/6 pass。詳細は一覧を参照。
