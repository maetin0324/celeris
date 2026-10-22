---
title: CoS のスレッドを統括 CoS と案件ごとの CoS に固定する
status: done
completed: 2026-10-22
updated: 2026-10-22
tasks: [01M5JB3QMBZ6BB6J18YB6BBC0J]
---
# CoS のスレッドを統括 CoS と案件ごとの CoS に固定する

- ADR: agent-docs/adr/2026-10-22-cos-fixed-threads.md（関連 ADR 7 本の冒頭に 2026-10-22 付記）
- store: migration 0092（`chat_threads.kind` を `general`／`project`／`legacy` に固定、部分 unique index、`read_through_seq`）。
  `cos_thread_ensure_tx`（唯一の作成経路）・`cos_thread_open`・`chat_thread_mark_read`、`ChatThread.{unread_count, attention_count}`。
- 振り分け: `CosTriageSource.task_id` → root の案件 → その CoS スレッド（無ければ統括）。root 完了要約も同じ。
  外部通知に `[統括 CoS]`／`[CoS: <案件>]` を付ける（escalation・fallback は packet の `cos`、root 完了は本文の先頭）。
- dispatcher: 受信箱スレッドの探索・origin card を消し、triage run をスレッドごとに起動。session は従来どおり DB の
  `node_sessions` で resume・rollover。
- API: `POST /api/v1/chat/threads` は `{"project_id"}` で固定スレッドを開く（冪等）、`POST /chat/threads/{t}/read`。
- web: ホームの一覧は統括 → 案件 → 旧会話、種類・要対応・未読の badge と合計、既定は統括 CoS、表示中の発言を既読にする。
  「新しい会話」・題名変更は消し、旧会話だけ保管できる。案件の画面に「この案件の CoS」。

## 証拠

- `cargo nextest run -p task-core`: 1055 passed（新規 `cos_chat_triage_items_and_escalations_follow_the_root_project`、
  `chat_migration_0092_maps_threads_without_losing_data`、`cos_thread_open_is_fixed_per_scope`、`cos_thread_read_watermark_counts_unread`）
- task-dispatch: `cos_chat_triage_routes_items_to_the_project_or_general_cos_thread`、
  `root_done_of_a_project_goes_to_the_project_cos_thread_without_new_threads`、
  `cos_chat_session_survives_daemon_restart_in_the_same_thread`、
  `cos_chat_session_expiry_after_restart_starts_fresh_from_summary_in_the_same_thread`
- web: `pnpm exec vitest run features/chat e2e/support api` 262 passed、`tsc -b`・`biome check .` exit 0
- 受け入れの検査（test-affected・clippy・fmt）の結果は報告本文に書く

## 未解決事項

- web の e2e（`e2e/chat/threads.spec.ts`・`cards.spec.ts`・`shell/home-layout.spec.ts` を新しい一覧に合わせて書き換えた）は
  release gate でだけ走る（ADR 2026-10-20-full-tests-at-release-only）。この run では走らせていない。
- 移行前の `pending`／`running` の受信箱項目は統括 CoS に付いたまま（ADR D6）。

## 本番への移行

config の変更は無い。migration 0092 は release の昇格で適用される。旧受信箱スレッドが統括 CoS（同じ id）になり、
人が作った会話と root 完了の旧スレッドは `legacy`（読むだけ）として一覧の末尾に残る。

## 付記（attempt 2、review の差し戻しへの対応）

- 指摘: `legacy_thread()` が legacy スレッドを作り続け（`POST /console/new-conversation`・`console.new_conversation` は毎回新規）、
  `POST /org/cos/messages`・`POST /console/instruct` は scope ごとの legacy thread に入っていた。legacy の読むだけは未強制。
- 対応（ADR D1・D2・D2'・D7・付記）: `POST /console/new-conversation`・`console.new_conversation`（surface registry と
  ADR 2026-10-09 の表）・`legacy_thread()`・`chat_legacy_default_thread`・`chat_legacy_new_conversation(_tx)` を削除。
  Console・org の CoS 入力は `start_console_cos` → `chat_console_enqueue` で固定 CoS スレッドへ。`message_post_conn`・
  `chat_message_post_cos_tx` は legacy を 409 で断る。旧 Console 行の投影は残した legacy スレッドがあればそこへ、無ければ固定スレッドへ。
  web: Console の「新しい会話」・`conversationStore`・`startNewConversation` を削除、ホームの legacy は composer の代わりに注記。
  api.md §3.109 と行 98、feature-parity R39 を廃止に更新。
- 試験: task-api `console_instruct.rs`（3 件を固定スレッド前提に書き換え、スレッド数の不変を追加）、`chat_api.rs`
  `chat_api_legacy_thread_is_read_only`、`console.rs`・`cos_ops_ops_surface.rs`・celeris-mcp から new-conversation を削除、
  task-core `chat_store_legacy_thread_is_read_only`・`chat_console_projection_*` 2 件、web `conversation-input.test.tsx` 2 件。

## 付記（attempt 3、決定的 check の失敗への対応）

- 失敗: release 相当の負荷下で `celeris::root_done_summary root_done_runs_cos_once_and_notifies_once` が `thread write` 2≠1。
  原因は試験側: 書き込み先が統括 CoS の固定スレッドになったので、受信箱 triage の run の返答も同じスレッドに入る。
  試験はスレッドの assistant 発言を全部数えていた。root 完了の入力（`root-done:%`）の run が書いた発言だけを数えるよう直した。
- 同じ調査中に、偽 webhook が要求を送らずに閉じた接続（host 上の別 process の port 探り）を空の本文として記録し、
  4 本とも落ちる回を再現した。要求の無い接続は記録しないようにした。実装（daemon 側）は変えていない。
- 証拠: `cargo nextest run -p celeris --test root_done_summary` を 8 回連続 4/4 pass。`bash scripts/dev/test-affected.sh` exit 0
  （6280 passed, 16 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 提案

- 統括 CoS の run 文脈に各案件 CoS の summary（`chat_threads.summary`）を短く渡すと、全案件の見渡しが厚くなる。
