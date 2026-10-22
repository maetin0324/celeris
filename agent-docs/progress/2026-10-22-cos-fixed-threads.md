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

## 提案

- 統括 CoS の run 文脈に各案件 CoS の summary（`chat_threads.summary`）を短く渡すと、全案件の見渡しが厚くなる。
