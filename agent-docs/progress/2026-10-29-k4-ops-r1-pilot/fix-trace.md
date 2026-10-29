---
title: ops-r1 B の trace と trial 別消費の読み取り専用 export
tasks: [01M62VP61K6XHRJW0H2YVX7N4D]
status: running
updated: 2026-10-29
---
# ops-r1 B の trace と trial 別消費の export

## design（WU design、2026-10-29 完了）

verdict は feasible。daemon（release 94f86928dc13、schema 版 102）のコード変更は要らない。

- 書いたもの: `docs/ops/ops-r1-trace-attribution.md`（6 段の取り元・reason の語彙・attribution の行の形と帰属規則・
  byte 一致の規則）と ADR `agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md`。crates/ は変えていない。
- 調べた schema: `chat_threads`・`chat_messages`・`chat_runs`（0050・0061・0092・0102）、`cos_operations`（0050）、
  `events`（0002）、`tasks`（`parent_id`・`root_id`）、`runs`（0026・0071）、`root_done_summaries`（0080）、
  `decision_log_usage`（0082、使わない）。CoS の `task.create` は `cos_operations` に `target_kind='task'`・`target_id`=新 task、
  監査 event は新 task の stream に入る（`task-core/src/chat/operations.rs`）。報告は root done のとき daemon が
  案件 CoS thread に積む依頼への CoS の返答（`task-dispatch/src/dispatcher/cos_chat/root_done.rs`）。
- DB に無いもの: condition_id と cutoff。どちらも引数で必須にする。
- 決めたこと: subcommand 名は `ops-r1-trace`・`ops-r1-attribution`（既存 `ops-r1-intake` に揃える）。
  受付 message は `--intake-record` の `message_id`、無ければ `client_message_id='ops-r1-intake:<trial_id>'`。

## 検査（提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh docs/ops/ops-r1-trace-attribution.md agent-docs/adr/2026-10-29-ops-r1-trace-attribution-export.md`: ok
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/`: 空

## 未解決事項

- ADR 2026-10-29-ops-r1-cos-intake-record（task 01M62TJ5V2B65Y0C804HHQ4E3Q）はこの branch に未取り込み。取り込まれたら ADR の関係欄をリンクにする。
- 統括 CoS が読むだけで操作しない turn は試行に結べず `no_trial_reference` で未帰属に残る。
- `chat_runs` の model を出すかは未確認（`resolved_config_json` の key を実装葉で確かめる）。今の行の形には model を入れていない。

## 提案

- 実装の葉（trace・attr）は試験を `crates/celeris/tests/ops_r1_trace*.rs`・`ops_r1_attr*.rs` に置き、
  `actor_trace/support.rs` に helper を足して fixture を作る。
