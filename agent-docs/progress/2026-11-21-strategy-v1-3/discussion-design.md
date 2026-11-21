---
title: 戦略層 v1 子 3 discussion-design — 案件の CoS Chat の discussion mode の ADR
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# discussion-design（WorkUnit。子 3 の段 design）

完了日: 2026-11-21。設計の記録は [ADR 2026-11-21-chat-discussion-mode](../../adr/2026-11-21-chat-discussion-mode.md)。
source（`crates/`・`web/`）は変えていない。

## 結論

- mode の欄は `discussion_mode`（`discuss`・`propose`・`execute`、型 `ChatDiscussionMode`）。既存の `ChatSendMode`（`mode`）とは別。人の message・relay・
  MCP の `console_instruct` の入力の `chat_messages.metadata_json` に、`discuss`・`propose` のときだけ書く。省略と `execute` は今と 1 byte も変わらない。
- run の mode は保存せず、前の完了した run より後の人の入力（`E(r)`）と run の入力から、最も厳しいもの（discuss > propose > execute）を導く。
- discuss と propose の run は `/cos/operations` の 111 の action をすべて 403 `discussion_mode_forbidden` で拒否し、`OperationAudit::reject` で監査する。
  `instructed_by` でも外れない。execute は今の検査（policy・instructed_by・human_required・basis・goal_guard・strategy_gate・authority_gate・domain）のまま。
- propose の案は返事の workspace file `strategy/<id>/v<n>.json`（`celeris.strategy/1`、`decision_state` は discussion か proposed）を chat の添付として残す。
  `knowledge.record` は日次の整理で正本に入るので使わない。
- 編集・撤回は人の principal（作者本人）だけ。前の本文は `chat_events` の `message_revised` に残す。要約の cursor と CoS の session を戻す。
- 引用は `{thread_id, message_id, revision, sha256}`。失効（current・edited・retracted・missing）は読むときに導き、保存しない。
- 新しい DB 表・migration・Manager・trait・Agent サービスは足さない（ADR D10）。

## 読んだ source（base はこの WU の `$CELERIS_WU_BASE`）

- `crates/task-core/src/chat/mod.rs`: `ChatMessage`・`ChatRelay`・`ChatRun`・`ChatEvent`/`ChatEventData`・`ChatPostMessageRequest`（`deny_unknown_fields`、`mode: ChatSendMode`）・`ChatRelayRequest`・`ChatActor`・`ChatSendMode`・`ChatEventType`
- `crates/task-core/migrations/0050_cos_chat.sql`: `chat_messages`（`metadata_json`）・`chat_events`（type に CHECK 無し）・`chat_attachments`・`chat_attachment_refs`・FTS5 の trigger `chat_search_message_update`
- `crates/task-core/migrations/0119_chat_queued_index.sql`: `idx_chat_messages_relay`（`metadata_json` の key を索引で読む前例）
- `crates/task-core/src/chat/human_post.rs`: `chat_message_post_as_also`・`chat_relay_post_also`・`chat_posted_project_tx`（human_epoch）
- `crates/task-core/src/chat/store.rs`: `MessageProvenance`・`insert_message`・`chat_message_cancel_tx`・`chat_mcp_instruct`・`append_event`
- `crates/task-core/src/chat/run_store.rs`: `chat_run_claim_next`（run の入力は 1 つの message）・`chat_message_attach_workspace_files`・`chat_events_retention`（消すのは `text_delta`・`tool` だけ）
- `crates/task-core/src/chat/credential.rs`: `chat_thread_checkpoint_at`（`summary_through_seq` と `expected_summary_through_seq`）
- `crates/task-core/src/chat/session.rs`: `chat_session_retire`・`chat_session_set_delivered_through`
- `crates/task-core/src/chat/attachments.rs`: 参照のある添付は GC で消えない（`NOT EXISTS chat_attachment_refs`）
- `crates/task-core/src/chat/intent.rs`: 冒頭の注記（型の推定は hint で認可に効かない）
- `crates/task-api/src/cos/operations.rs`: `allowed`・`OperationAudit`・`OperationAudit::reject`・`OperationBody`・`create_operation`（検査の順）・`bind_run_policy`
- `crates/task-api/src/cos/operations/instructions.rs`: `instructed_message`
- `crates/task-api/src/cos/ops/{mod,tasks,decisions,projects,admin,surface}.rs`: `REGISTRIES` と各 `ALLOWED`（111 行。ADR D2.4 の表は script で生成）
- `crates/task-api/src/chat/mod.rs`: `routes`・`post_message`・`post_relay`（operator だけ）
- `crates/task-api/src/principal.rs`: `Principal`
- `crates/task-api/src/knowledge.rs`: `tree`・`page`（CoS credential の口は GET だけ）
- `crates/task-api/src/cos/{inbox,review,knowledge_use,mod}.rs`: CoS credential の口の route
- `web/features/chat/`（composer・data・messages）: 既存の送信の `mode: "queue" | "interrupt"`
- `agent-docs/progress/2026-11-21-strategy-v1-0/design-brief.md` §1・§5・§7（X5・T2・T5）、`agent-docs/adr/2026-11-21-strategy-evidence-contract.md` D1〜D6

## 証拠のコマンドと結果（提出時の HEAD で実行）

- ALLOWED の抽出: `awk '/^pub\(crate\) const ALLOWED/,/^\];/' crates/task-api/src/cos/ops/<registry>.rs | tr -d '\n' | sed 's/),/)\n/g' | grep -o '"[A-Z]*", *"[^"]*", *"[^"]*"'` → 5 つの registry で計 111 行。
- `struct literal` の数: `grep -rn "ChatPostMessageRequest {" crates --include=*.rs | wc -l` → 65、`ChatRelayRequest {` → 20（実装 WU が `discussion_mode: None` を足す数）。
- `cargo` の試験・clippy・fmt はこの WU では走らせていない（文書だけの変更。source を変えていない）。

## 未確認の事項

- 本番で `chat` 参照を持つ `celeris.strategy/1` 文書が 0 件であること（子 2 が main に入ってからの本番の release に含まれるかを含め、本番 DB を読んでいない）。D3 の欄の追加はこれを前提にする。
- MCP の client（Muse など）に discuss を強いるか、`console_instruct` の scope をどうするか（子 4 が決める）。
- CoS の harness の道具（shell など）による Celeris の外の副作用は、この ADR の保証の外。CoS の tool policy を mode で狭めるかは決めていない。
- dispatcher で workspace file を添付する呼び出し元の正確な file（`chat_message_attach_workspace_files` を呼ぶ所）は読んでいない。summary-cite WU が確かめる。
- `crates/task-api/src/cos/review.rs` の `report_direct` が CoS credential で何を書くかは読んでいない（action `chat.review_report` として表に含めた）。

## 提案

- 子 7: chat の案（`strategy/<id>/v<n>.json` の chat の添付）を Web で採用・却下する口は、子 2 の決定の経路（task の成果物と `strategy-approve-v<n>`）に
  つなぐ形で決める。新しい承認の経路を作らない。
