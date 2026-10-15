---
tasks: [01M4YES0YP2F68ZG10RYANHYHB]
status: done
completed: 2026-10-15
---
# root 完了まとめ: 事実収集・fallback 文・NotificationKind（WU facts）

ADR: [2026-10-15-root-done-cos-summary](../../adr/2026-10-15-root-done-cos-summary.md) D2/D4/D6。

## やったこと

- `crates/task-core/src/notify.rs`: `NotificationKind::RootDoneSummary`（`root_done_summary`）を追加（`as_str`・`parse`・`ALL` 16 件）。
  DB の `notifications.kind` に CHECK は無いので migration 不要。schema（`docs/api/v1/api-v1.schema.json`）・web の生成型
  （`web/api/generated/{schema.json,types.ts}`）を再生成、`docs/api/v1/api.md` の kind 一覧に追記。
- `crates/task-ops/src/root_done.rs`（新規。task-dispatch と celeris の両方から使えるよう task-ops に置いた。LLM・I/O なし）:
  - `is_root_done`（done・`parent_id` なし・木の子でない・support でない）— 対象判定はここ 1 か所。
  - `done_transition_seq`・`summary_key`（`<task_id>:<seq>`）。
  - `collect`（task・events・delivery・子 task・進捗ファイル本文 → `RootDoneFacts`: 段の統合、統合 commit〈`integration_merged`、
    無ければ ready かつ release 付き delivery の head〉、release、成果物 path、受け入れ判定、`delivery_skipped`、本番への移行節の要否、
    worker 申告〈参考のみ〉）。
  - `progress_mentions_task`（front matter `tasks:`）・`progress_sections`（「なし」「不要」だけの節は無しと見る。code block 内の `## ` は無視）。
  - `facts_block`（CoS スレッドの system message）・`fallback_body`（Discord 短文。`web_base_url` が空ならエラー）・`task_url`・`thread_url`。
- 旧 `TaskReady` は消していない（次の WU wire が配線と同時に置換する）。

## 証拠

- `cargo test -p task-ops --lib root_done` → 6 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest 5137 passed / 0 failed（summary: passed 5140, failed 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項

- 進捗ファイルを統合 commit の tree から引く（`git show`）部分と子 task の読み出しは呼び出し側（wire）に残る。

- 範囲 check（docs/architecture-map.md は許可外）のため、architecture-map への root_done 行は closeout 葉で足す。
