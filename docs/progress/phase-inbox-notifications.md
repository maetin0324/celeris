# 受信箱と通知の 2 系統（ADR-0133、task 01M3YFCJKMNWQ13HRS52M5BSWW）

- 完了日: 2026-10-02（WorkUnit verify、HEAD `0bc7ca75a5a8`、schema 40）。
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
- migration 番号の重複確認: `crates/task-core/migrations/0040_feed_notices.sql` を持つのはこの task の
  ブランチ（`celeris/01M3YFCJKMNWQ13HRS52M5BSWW`）だけ（`git branch -a --contains` で確認）。0038/0039 は
  他ブランチ（work_unit_sessions・cron_jobs・write_sets）が予約済みのためスキップ済み（migration 冒頭コメント
  に理由を記載）。main には今のところ 0037 までしか無い。

## task 01M3YF3NS2EGTZD2BBWNPG1K28（inbox-rules）との結合状況

- **main には未統合**。`git log main --oneline | grep inbox-rules` は 0 件。該当コミット
  `3c967c91 integrate wu/inbox-rules (phase core)`（ADR-0131 D7、`attention_suppression` / `SuppressRule`
  / `Inbox.suppressed` を `crates/task-ops/src/inbox.rs` に実装）は `celeris/01M3YF3NS2EGTZD2BBWNPG1K28`
  ブランチにのみ存在し、main の祖先ではない（`git branch -a --contains 3c967c91` → そのブランチのみ）。
- **この task のブランチにも未統合**（`git merge-base --is-ancestor 3c967c91 HEAD` → no）。
- 結果として `crates/task-ops/src/human_inbox.rs` は ADR-0133 D4 で決めたとおり、`attention_suppression` を
  呼ばず `build_attention` の現行出力（抑制規則なし）をそのまま使う**仮実装のまま**（コードのコメントに
  依存関係を明記済み。規則の仮実装は置いていない）。
- **未解決**: 「置き換え済み failed 子・終端 task の attention を外す」規則は、inbox-rules が main（または
  この task 系列のブランチ）に統合されるまで新しい受信箱 API（`GET /inbox/items`）に反映されない。
  そのため「置き換え済み failed 子が新しい受信箱 API に出ないこと」を確かめる既存試験は無い（書けない）。
  inbox-rules が main に入り次第、`human_inbox` 側を `attention_suppression` を使うように差し替える小さな
  追従 WorkUnit が必要（この task では規則を重複実装しない。人の追記どおり inbox-rules 葉に一本化）。

## ADR-0133 D6（外部送り出し）の既知のギャップ

- 決定的な判定・束ね（`inbox_new` / `digest`）・`[notify]` の 4 設定キー（`inbox_batch_secs` 既定 60 /
  `inbox_reminder_secs` 既定 86400 / `digest_interval_secs` 既定 3600 / `digest_max_lines` 既定 10）は
  `crates/celeris/src/notify.rs` に実装・試験済み（`notify/tests.rs`、`dispatcher/tests/tick_and_dispatch.rs`）。
  `cargo test --workspace` に含まれ全 pass。
- ただし ADR 本文「`GET /api/v1/notify` にこの 4 値と、2 経路それぞれの最後の送信時刻を載せる」は未実装:
  `crates/task-api/src/notify.rs` の `NotifyView` は `configured` / `secret_id` / `fingerprint` /
  `gui_base_url` / `recent` のみで、4 設定値・経路別の最終送信時刻を持たない（grep で確認、該当フィールドなし）。
  本番の設定確認は今のところ `config/*.toml` を直接読むしかない（下記 docs/ops 参照）。
  **未解決**: `NotifyView` に 4 値 + 経路別 `last_sent_at` を足す小さな追従 WorkUnit が必要。

## 提案

- 上記 2 件の未解決事項はどちらも独立した小さい追従 WorkUnit で閉じられる規模（1）inbox-rules 統合後に
  `human_inbox` を `attention_suppression` へ差し替え、「置き換え済み failed 子が出ない」試験を追加する、
  2）`NotifyView` に 4 設定値と経路別最終送信時刻を足す）。どちらも ADR-0133 の変更は不要（既存の決定の範囲内）。
