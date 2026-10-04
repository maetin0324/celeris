---
title: 受信箱と通知の 2 系統（ADR-0133）
tasks: [01M3YFCJKMNWQ13HRS52M5BSWW]
status: done
updated: 2026-10-03
---
# 受信箱と通知の 2 系統（ADR-0133、task 01M3YFCJKMNWQ13HRS52M5BSWW）

- 完了日: 2026-10-02（WorkUnit verify、HEAD `0bc7ca75a5a8`、schema 40）。追従の rules-wire / notify-status /
  sync-main で未解決 2 件を閉じ、最新 main を取り込み schema 41 に振り直した（末尾の節）。
- 対象: adr / inbox-model / notify-store / notify-feed / api / outbound / gui-compat（全 [done]）。web 葉は
  UI/UX task `01M3XTCNKMQBCHKSZ7Y1GF6ZM4` の決定（`ui-overlap = c`）により `[superseded]`。

## 検査結果

- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（警告なし）。
- 今回の追試: `cargo test -p task-core -p task-ops -p task-api -p celeris --lib` → exit 0
  （全 unit test pass。`human_inbox` と `notify` の試験を含む）。
- `cargo test -p task-core -p task-ops -p task-api -p celeris` → exit 101。lib 試験は pass したが、
  `crates/celeris/tests/instance_handoff.rs` で次の 5 件が失敗した。`verify_mode_never_dispatches_and_never_touches_daemon_instances`、
  `normal_mode_does_not_inject_the_smoke_builtins`、`starting_the_same_release_twice_exits_three` は worker DB guard の
  user namespace probe が sandbox の `Operation not permitted` で失敗。これらの試験には
  `CELERIS_ISOLATION_TESTS=skip` の既存分岐がないため skip を偽装せず失敗として記録した。
  `a_newer_release_takes_over_while_the_old_one_finishes_its_run` と `a_stale_heartbeat_promotes_the_standby` は
  handoff の wall-clock 条件を満たさず失敗した（負荷時の既知の時間依存 flaky）。いずれも本 task の変更範囲外。
- `cargo test --workspace` → exit 0。121 バイナリ・doctest すべて `ok`（**3,294 passed / 0 failed**、ignored は
  既存の手動試験〈実 sccache・実クラスタ・timing evidence〉のみ）。
- 再実行 1 回で `task-worker` の `scratch::tests::wrapper_runs_the_compiler_directly_when_the_server_is_unreachable`
  が `Text file busy`（ETXTBSY）で 1 回だけ失敗（`crates/task-worker/src/scratch/tests.rs:835`）。単体では
  `cargo test -p task-worker --lib scratch::tests::wrapper_runs_the_compiler_directly_when_the_server_is_unreachable`
  → `ok`（再現せず）。この試験はローカルの `write_script` ヘルパーで実行ファイルを書いており、
  `crate::test_support::write_executable`（既知の ETXTBSY 回避策、他の task-worker 試験で使用済み）を
  使っていないため、並列実行下での exec と書き込みの競合で稀に ETXTBSY になる。**この task の変更
  （task-core/task-ops/task-api/gui）の範囲外**（task-worker の sccache wrapper 試験）であり、本 task では
  未修正（別 task の対象）。
- workspace の既存実行では browser isolation 試験は pass し、skip は使わなかった。一方、今回の関連 crate 追試では
  上記 3 件の worker DB guard 試験が sandbox の user namespace 制限で失敗した。
- migration 番号の重複確認（verify 時点）: 当時は 0040 を選んだが、sync-main 葉の再走査で 0040 も他ブランチ
  （behind_targets）が使用中と分かり 0041 へ振り直した（末尾の節）。

## task 01M3YF3NS2EGTZD2BBWNPG1K28（inbox-rules）との結合状況

- verify 時点では main にもこのブランチにも未統合だった（`3c967c91` はそのタスクのブランチのみ）。
- **rules-wire 葉で結合済み**: そのタスクの `788e5cc0`・`739cd209` を `git cherry-pick -x` で内容を変えずに取り込み
  （このブランチの `966dd2b1`・`559bb4ab`、定期実行 task 側と同一差分）、`human_inbox` の自動で閉じるを
  `task_ops::inbox::attention_suppression` に結合（`10504208`、`suppressed` に規則別件数）。規則の重複実装はしていない。
- 試験: `crates/task-api/tests/inbox_notifications.rs` の
  `auto_close_drops_meaningless_items_and_keeps_failed_needing_a_decision`（置き換え済み failed 子・終端 task の
  attention が新しい受信箱 API から消え、判断が要る failed は残る）、task-ops の `inbox_cleanup_*`。

## ADR-0133 D6（外部送り出し）

- 決定的な判定・束ね（`inbox_new` / `digest`）・`[notify]` の 4 設定キーは `crates/celeris/src/notify.rs` に実装・試験済み。
- verify 時点で未実装だった `GET /api/v1/notify` への 4 設定値（`inbox_batch_secs`・`inbox_reminder_secs`・
  `digest_interval_secs`・`digest_max_lines`）と経路ごとの最終送信時刻は **notify-status 葉で実装済み**（`988ce389`、
  試験 `crates/task-api/tests/notify.rs` の `get_notify_status_exposes_route_settings_and_last_successful_sends`）。

## sync-main 葉（2026-10-02）: main の取り込み・migration の振り直し・再検査

- 取り込み: `git merge main`（main `5d6df9f3`、作業中に進んだ `14b052ea`・`33aca5a3` も続けて merge（後者は衝突なし）、rebase なし）。衝突は `docs/PROGRESS.md` の 1 箇所だけで、
  この task の節と main の「codex・opencode への skill の付属ファイル」節を両方残して解消。
  `git merge-tree --write-tree main HEAD` → exit 0。
- migration の振り直し: `git for-each-ref refs/heads/celeris/` の全ブランチを `git ls-tree` で走査し、
  0038（work_unit_sessions、6 ブランチ）・0039（cron_jobs 1・write_sets 4）・0040（behind_targets 4）が他ブランチで
  使用中、0041 以上は未使用、main は 0037 まで。`git mv` で `0040_feed_notices.sql` → `0041_feed_notices.sql`、
  `MIGRATION_0041`・`SCHEMA_VERSION = 41`・`RESERVED_VERSIONS = [38, 39, 40]`、`feed/tests.rs` のコメント、
  `store/tests.rs`・`cluster_job/tests.rs` の `SCHEMA_VERSION` 固定値 9 箇所を 41 に更新。
- 結合の修正: cherry-pick した inbox-rules（ADR-0131 D7）で人の Cancel が failed → cancelled を許すようになり、
  `tests/e2e/tests/phase7_scenarios.rs` の `cancel_is_limited_to_non_terminal_tasks_and_failures_cancel_dependents`
  が古い期待（failed の中止は exit 1）で落ちた（定期実行 task のブランチにも同じ古い期待が残っている）。
  終端の拒否は cancelled task で確かめ、failed の中止は成功して `Failed->Cancelled:cancel_failed`・attempts 不変を
  確かめる形に更新。
- 検査: `cargo fmt --all -- --check` exit 0。`cargo clippy --workspace --all-targets -- -D warnings` exit 0。
  `cargo test --workspace` exit 0（**3,406 passed / 0 failed / 14 ignored**、`33aca5a3` 取り込み後の最終実行）。
  範囲外の flaky はこの実行では出なかった。

## sync-main 再検査（2026-10-03）

- 前回の `cargo test --workspace` check は task-dispatch の
  `cluster_job_wait::a_wait_parks_the_task_polls_and_resumes_as_a_continuation` が 60 秒の状態待ちで exit 101。
  同じ test を単独で再実行した結果は 1 passed / 0 failed。全体再実行でも同 test は pass した。
- sandbox 内の全体再実行は `crates/celeris/tests/instance_handoff.rs` の 5 件で exit 101。
  3 件は worker DB guard の user namespace probe が `Operation not permitted`、残る 2 件は引継ぎ条件に
  達しなかった。sandbox 内の `unshare -U -r true` も exit 1、隔離外では exit 0。隔離外の
  `cargo test --workspace` は exit 0（**3,406 passed / 0 failed / 14 ignored**、126 件の test result 行）。
  `cargo fmt --all -- --check` と `cargo clippy --workspace --all-targets -- -D warnings` も exit 0。
  この再検査ではコードを変更していない。

## 提案

- 定期実行 task（`01M3YF3NS2EGTZD2BBWNPG1K28`）のブランチにも `phase7_scenarios.rs` の古い期待が残っているので、
  そちらの統合でも同じ更新が要る（この task のブランチを先に main へ入れれば merge で揃う）。

## main から入った追記（sync-main-2 で移した）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## 受信箱と通知の 2 系統（2026-10-02、task 01M3YFCJKMNWQ13HRS52M5BSWW、ADR-0133、WorkUnit verify）

- 完了日: 2026-10-02。全葉（adr / inbox-model / notify-store / notify-feed / api / outbound / gui-compat）完了、
  ADR-0133 の状態を「実装済み」に更新（web 葉は人の決定 `ui-overlap = c` で UI/UX task
  `01M3XTCNKMQBCHKSZ7Y1GF6ZM4` へ `superseded`）。詳細・証跡は
  [phase-inbox-notifications.md](2026-10-02-inbox-notifications.md)。
- 証拠（HEAD `0bc7ca75a5a8`、schema 40）: `cargo fmt --all -- --check` exit 0。
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0。`cargo test --workspace` exit 0
  （**3,294 passed / 0 failed**、121 バイナリ + doctest、ignored は既存の手動試験のみ）。再実行 1 回で
  `task-worker` の `scratch::tests::wrapper_runs_the_compiler_directly_when_the_server_is_unreachable` が
  ETXTBSY で単発失敗（単体実行では再現せず、この task の範囲外の既知の flaky）。今回の関連 crate 再試験は
  `--lib` で全 pass。integration test 込みでは `instance_handoff.rs` の 5 件が失敗: 3 件は worker DB guard の
  user namespace probe が sandbox の `Operation not permitted`、2 件は handoff の wall-clock 条件（既知の flaky）。
  この 3 件に既存の `CELERIS_ISOLATION_TESTS=skip` 分岐は無く、失敗として記録（詳細は上記 progress 文書）。
- 追従（rules-wire / notify-status / sync-main、2026-10-02）: verify 時点の未解決 2 件は閉じた。1) inbox-rules
  （task `01M3YF3NS2EGTZD2BBWNPG1K28` の `788e5cc0`・`739cd209`）を `cherry-pick -x` で取り込み `human_inbox` に結合
  （試験 `auto_close_drops_meaningless_items_and_keeps_failed_needing_a_decision`、規則の重複実装なし）。2) `GET /api/v1/notify`
  に D6 の 4 設定値と経路別最終送信時刻（試験 `get_notify_status_exposes_route_settings_and_last_successful_sends`）。
- sync-main: 最新 main（`5d6df9f3`、続けて `14b052ea`・`33aca5a3`）を `git merge`（衝突は本ファイルだけ、両方の節を残して解消、`merge-tree` exit 0）。
  全 celeris/* の走査で 0038〜0040 が他ブランチ使用中のため `0040_feed_notices.sql` を `0041` へ `git mv`
  （`SCHEMA_VERSION = 41`、`RESERVED_VERSIONS = [38, 39, 40]`）。inbox-rules の「人は failed を cancel できる」に合わせ
  e2e `phase7_scenarios` の期待を更新。証拠: `cargo fmt --all -- --check` exit 0、
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0、`cargo test --workspace` exit 0（**3,406 passed / 0 failed**）。
- sync-main 再検査（2026-10-03）: 前回 check の `cluster_job_wait::a_wait_parks_the_task_polls_and_resumes_as_a_continuation`
  は 60 秒の状態待ちで失敗したが、単独再実行と今回の全体実行ではともに pass。sandbox 内の
  `cargo test --workspace` は別の `instance_handoff` 5 件で exit 101（worker DB guard の user namespace probe が
  `Operation not permitted`、残り 2 件は引継ぎ条件に達せず）。隔離外では `unshare -U -r true` exit 0、同じ
  `cargo test --workspace` exit 0（**3,406 passed / 0 failed / 14 ignored**）。`cargo fmt --all -- --check` と
  `cargo clippy --workspace --all-targets -- -D warnings` は exit 0。コード変更なし。詳細は進捗文書を参照。
- 本番 host で人が確認・設定する手順は [docs/ops/inbox-notifications.md](../../docs/ops/inbox-notifications.md)。


## 通知フィード同期の退行修正（release 3527c8e39ee2 の verify 失敗、2026-10-03）

- 原因: 29d0ffc5 が tick ごとに `sync_notifications`（5982b3cb）を呼ぶ。その同期は events を全部読むまで回り、1 行ごとに
  走査位置を書いていた（写し 172,994 行で 11.3 秒・書き込み 17 万回。NFS の staging では位置 2170 で止まり dispatch に戻らない）。
  追いついた後も、tick ごとに報告・発言・delivery の全件に書き込みの transaction を開いていた（約 500 回/tick）。
  詳細と決定は ADR-0133 付記「通知フィードの同期を差分にする」。
- 修正: 走査位置（events id・報告/発言の created_at）からの差分だけを読み、events は 1 回 2048 行まで。記録済みは
  読み取り接続で先に一括判定し、新しい出来事の記録と位置の更新を 1 つの transaction で書く（同期 1 回の書き込み高々 1 回、
  何も無ければ 0 回）。通知・delivery の読み取りは読み取り接続へ。
- 測定（staging の写し、`crates/task-ops/tests/feed_measure.rs`）: 修正前 初回 11.3 秒・以後 tick ごと約 500 回の書き込み →
  修正後 初回 113 ms・書き込み 1 回、追いついた後 1〜2 ms・書き込み 0 回。記録結果（feed_sources 502 件）は同じ。
  verify の起動と煙試験を写しで再現（artifacts の replay-verify.sh）: 修正後 smoke done 8.1 秒・slow api 0 件。
- 回帰試験（件数で固定）: `task-ops notify_feed::tests::notify_feed_sync_reads_only_new_sources_and_writes_nothing_when_idle`、
  `task-api --test request_lock_counts`（300 task × events 1,800 / 21,300 行で inbox・notifications・org・tasks/{id} の
  接続回数が同じ、書き込み接続の上限）、`task-dispatch tick_feed_sync_is_bounded_and_idle_ticks_do_not_scale_with_events`。
- 証拠: `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0、
  `cargo test -p task-ops` 431 passed、`-p task-api` 432 passed、`-p task-core` 652 passed、`-p task-dispatch` 514 passed。
- 未解決: 受信箱の構築は task ごとに events を読む既存の形のまま（読み取り接続で task 数に比例）。本番 active の
  tick loop は 30 秒ごとに `notify::schedule_routes` で受信箱を構築する（verify の退行とは別）。
- release/verify（2026-10-03 02:42〜02:51 UTC、検証済み sha `8e42a33ca126aa5f75acae7aad966dfbad018537`）: run の sandbox では
  `~/.local/celeris/releases` が読み取り専用の mount なので、`CELERIS_STATE_DIR` を run の artifacts の別 dir
  （`current` は本番の `releases/0b8a225629fd` への symlink、`SD_CELERISCTL=/nonexistent` で scratch lease を使わない、
  staging port は 17711/17701/17712）にして同じ台本を実行した。本番の DB は `.backup`（mode=ro）で読むだけ、昇格はしていない。
  - `scripts/selfdeploy/release.sh 8e42a33c`: exit 0（gate.json ok=true。fmt・cargo-test・clippy・build・pnpm の
    typecheck/test/build・mobile-audit・e2e-mock がすべて exit 0。web 段は既定の `SD_GATE_SKIP_WEB=1` で skip）。
  - `scripts/selfdeploy/verify.sh 8e42a33ca126`: exit 0、**verify.json ok=true**。検査 1 schema 41・2 件数一致（tasks 699）・
    3 主要 GET・4 GUI（/ と /projects/<id> を含めて 200）・4b gui-e2e・6 smoke（done 6.3 秒）がすべて true。
    live_ok=false は検査 5（N-1 の 0b8a2256 が schema 41 の DB を開けない `SchemaTooNew`）で、0037→0041 の migration を
    含む release では予期どおり（ok には入らない）。
  - staging log の slow api request（1 秒超）: 修正前 31 件・最大 13.8 秒（org・browser/waits・inbox・integrations）→
    修正後は最大 1.56 秒（inbox 3 件、gui-e2e 中に並行で読まれる tasks/{id} 244 件が 1.0〜1.45 秒）。org・browser/waits・
    integrations・notifications は 1 秒を超えなかった。tasks/{id} の 1 秒台は未解決として残す。
  - 本番の `~/.local/celeris/releases/8e42a33ca126` は作っていない。昇格の前に、人が host で
    `scripts/selfdeploy/release.sh 8e42a33c` → `scripts/selfdeploy/verify.sh 8e42a33ca126` を実行する。


## 通知フィード退行修正の main 取り込みと再検証（2026-10-03）

- merged-main: `d8e4c76290a7ec7c09738ee32a854fd39127616f`。
  final review の取り込み不可を解消するため、task branch に main を merge した。
  衝突は本ファイルのみ。通知フィード修正・release/verify と main 側 ADR-0129 の記録を両方全文残した。
  通知フィード修正のコード・回帰試験・ADR は merge 前と同一で、その他のコードは main と同一。
- merge 後の指定ゲート
  `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p task-ops && cargo test -p task-api`
  は exit 0（task-ops 431 passed、task-api 432 passed）。通常 sandbox での初回は既存の
  `browser_h3_injection::production_h3_injects_once_without_exposure` が `unshare: Operation not permitted`
  で停止したが、`require_escalated` で指定ゲート全体を再実行して通過。試験の除外・変更はしていない。
- `cargo test -p task-dispatch --lib tick_feed_sync_is_bounded_and_idle_ticks_do_not_scale_with_events`
  も exit 0（1 passed）。ログは run artifacts の `attempt3-gate.log` と `attempt3-tick.log`。
- release/verify の検証済み SHA は前節の `8e42a33ca126`（verify ok=true、smoke done 6.3 秒）。
  今回は main 取り込み後の指定ゲートと tick 回帰試験を再検証したもので、統合後 SHA の release/verify は再実行していない。
  本番への昇格・設定変更は行っていない。
