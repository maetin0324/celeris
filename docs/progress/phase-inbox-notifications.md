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
