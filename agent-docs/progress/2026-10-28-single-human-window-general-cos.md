---
title: 人が話す窓口を統括 CoS の 1 本にし、案件 CoS の escalate を統括に集め、統括から案件へ取り次ぐ
updated: 2026-10-28
status: done
tasks: [01M60ABQN55C65R4W13ZS0347F]
---
# 人が話す窓口を統括 CoS の 1 本にする

完了日: 2026-10-28。
基準: `git merge-base HEAD main`。

## 変更

- [ADR 2026-10-28-single-human-window-general-cos](../adr/2026-10-28-single-human-window-general-cos.md)（実装の前に書いた）。
  付記: [2026-10-22-cos-fixed-threads](../adr/2026-10-22-cos-fixed-threads.md)、
  [2026-10-27-relayed-instructions](../adr/2026-10-27-relayed-instructions.md)。
  [2026-10-09-cos-operations-all-mutations](../adr/2026-10-09-cos-operations-all-mutations.md) の表に `POST /chat/threads/{t}/forwards` を足した。
- 札（D2）: `task-core` の `chat/window.rs`。`cos_triage_outbox_claim`（escalate・fallback）と `cos_triage_direct_escalate` の
  transaction で、案件スレッドの item なら統括スレッドに assistant の発言を 1 件書く（`client_message_id` は
  `cos-escalation-card:<kind>:<key>:<revision>`）。`ChatMessage.escalation`（`ChatEscalationRef`）とカードの state は
  読むたびに `cos_inbox_items.state` から決める（閉じたら `resolved`）。migration なし。
- 取り次ぎ（D3）: CoS operation `chat.relay_forward`（`POST /api/v1/chat/threads/{t}/forwards`、直接の呼び出しは 403
  `chat_forward_cos_only`）。統括スレッドの run だけが、統括スレッドの人の発言（`author` が `human:*`、取り次ぎでない）の
  原文と `created_at` を案件スレッドへ queued の relay として写す。task 1〜20 件と期限が必須。`relay.relayed_by = cos:<統括>`、
  `relay.source = {thread_id, message_id, author}`。`instructed_by` の payload は `decided_by = {human, <name>, relayed_by: cos_general}`、
  `executed_by = cos_project`。
- 排他（D4）: 統括スレッドの run が案件の対象に `instructed_by` 付きで operation を出すと 422 `cos_instruction_invalid`。
- prompt・skill（D6）: `cos-inbox-triage` §10 を足し policy_version を 5 に（`[cos.triage] policy_version` の既定も "5"）。
  `cos-operator` の operation 表に `chat.relay_forward`。worker protocol の `CosChatInboxItem.thread_id` を足し、統括 CoS の
  prompt の受信箱の件に「案件スレッド: <id>（chat.relay_forward で取り次ぐ。自分では答えない）」を出す。
- web（D5）: ホームは統括 CoS を開く。スレッドの一覧（`threads.tsx`、drawer、検索、旧会話の保管）とその試験・browser 試験の
  fixture を消した。`ThreadsModel` は load・open・markRead だけにし、web の `patchThread` を消した。案件スレッドを開くと
  「読む用です。話すときは統括 CoS で。」と「統括 CoS に戻る」を出す。札には「元のスレッドを開く」（`/?thread=<案件スレッド>`）。
  e2e（`threads.spec.ts`・`mobile.spec.ts`・`cards.spec.ts`）から一覧の前提を消し、新しい導線の確認に置き換えた（e2e は release gate で走る）。
- 生成物: `docs/api/v1/api-v1.schema.json`、`docs/protocol/worker-protocol.schema.json`、`web/api/generated/*`。

## 試験

- `task-core` `chat::window_tests`: 札が source triple で 1 件だけ・統括の item には札なし・直送も 1 件・reconcile で解消表示、
  forward の原文・時刻・範囲（task なし・期限なし・期限切れ・別案件の task・人以外の発言・統括以外から・案件以外へ）。
- `task-api` `tests/single_human_window.rs`（LLM なし。試験が各 CoS run の credential で API を呼ぶ偽 harness）: 案件 CoS の escalate →
  統括に札 1 件（再送で増えない）→ 人が統括で回答 → 統括 CoS の直接回答は 422 → forwards の直接呼び出しは 403 →
  別案件への forward は 422 → `chat.relay_forward` → 案件 CoS の run の入力が取り次ぎ → 範囲外の task は 422 →
  `instructed_by` 付き回答（decided_by・executed_by）→ 期限切れの取り次ぎは 422 → 導出した受信箱で reconcile → 札が解消表示。
- web: `features/chat/home/chat-home.test.tsx`、`features/chat/threads/model.test.ts`。

## 証拠（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh` → exit 0（結果は報告本文）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 本番への移行

- config の変更は不要。本番 config に `[cos.triage] policy_version = "4"` が明示されていれば "5" に変える（外部 operator が利用者に告知して行う）。
- CoS の skill は本番 KB の `skills/<name>` から読まれる。`cos-inbox-triage`（§10、version 5）と `cos-operator`（operations.md）の
  更新を本番 KB に取り込む必要がある（人の import）。取り込むまで CoS は古い skill で動き、札の取り次ぎ方を知らない。
- DB の migration は無い。既存の escalated item には札を後から作らない。

## 未解決事項

- 統括 CoS が `GET /chat/threads/<t>/messages` で案件スレッドを読めるかは既存の CoS の読み取り権限に従う（この変更では触っていない）。
- `cargo clippy --workspace --all-targets` は `llm-proxy/tests/proxy_integration.rs` の `clone_on_copy` で落ちる（main 由来、範囲外）。

## 提案

- 人が札のボタンで直接答えた後に、案件 CoS へ「人が答えた」ことを知らせる system 行を案件スレッドに書くと、
  案件 CoS の文脈が揃う（今は案件 CoS が次の run の受信箱の件で知る）。
