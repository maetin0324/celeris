---
title: task-core の thread 種別 review・migration 0102・報告の書き込み関数
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# task-core の thread 種別 review・migration 0102・報告の書き込み関数

完了日: 2026-10-28

ADR [2026-10-28-cos-review-back-thread](../../adr/2026-10-28-cos-review-back-thread.md) の D1・D4'・D7 の task-core 部分。

## 変更

- `phase 1: task-core に review thread と報告の書き込み関数を足す` — 次の内容。
  - `ChatThreadKind::Review`（値 `review`）。migration 0102 は 0092 と同じ手順で `chat_threads` を作り直し、CHECK に
    `review` と「review は project_id を持たない」を足す。行・index・検索の trigger は保ち、部分 UNIQUE index
    `idx_chat_threads_review` を足す。thread の行は作らず、`chat_messages` は触らない（D7）。`SCHEMA_VERSION` は 101。
  - `crates/task-core/src/chat/review.rs`: `cos_review_thread_ensure_tx`・`SqliteStore::cos_review_thread_open`（冪等）・
    `cos_review_thread_get`（作らない。D6 の `GET /cos/review-thread` 用）・`cos_review_input_post`（cron の入力。key は
    `cron-cos-review:` で始まる。同じ key の再送は同じ message）・`cos_review_report_tx`（D4'。統括 thread に assistant
    1 件、key `cos-review-report:<入力 id>`、同じ入力の再送は最初の報告を `created=false` で返し何も書かない、
    `provenance` 付き、queue に入らないので統括 run は起きない、未読には数える。呼び出し側の transaction で走る）。
  - 人の画面から外す: `chat_thread_list`（一覧・検索。未読の合計は一覧の和）は既定で `kind<>'review'`。
    `ChatThreadQuery.include_review` を足し、dispatcher の queue の claim（`cos_chat/launch.rs`）と run の制御
    （`cos_chat/control.rs`）だけが true にする（false のままだと review thread の run が起きない）。
  - 投稿の拒否: `ChatError::ReadOnly`（403）を足し、task-api の `chat_problem`・`cos_problem` で code
    `chat_thread_read_only` に写した。review thread への `chat_message_post(_as)`・`chat_relay_post`・
    `chat_mcp_instruct`（thread 指定）・`chat_thread_patch`・`chat_thread_resume_queue`・`chat_message_post_cos_tx` を拒否する。
  - wire: `ChatMessage.provenance`（`ChatProvenance {kind: cos_review_report, review_thread_id, review_message_id, run_id}`、
    metadata_json の `provenance`）。API schema と web の生成型を再生成した。
  - 主体の写像: `decision_log` の `cos:<review thread>` と `cos_deployment_actor` は統括（CosGeneral / `cos_general`）。
    `instructed_by` は review thread では受けない。`cos_thread_label` は review では None（通知しない）。
  - 試験: `chat/store_tests.rs` に 3 件（1 件だけ・冪等 open・一覧/検索/未読に出ない、全投稿経路の 403 と入力の冪等と
    claim、報告は入力 1 件に 1 件・provenance・統括 run が起きない・不正入力 422）。`chat/tests.rs` に migration 0102 の
    試験（schema 100 の DB の threads・messages・chat_search の全行が変わらない、review の行を作らない、index・CHECK・
    trigger が効く）。固定の版数を持つ既存の試験を 101 に上げた（0100 のときと同じ箇所）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `cargo nextest run -p task-core -E 'test(cos_review) | test(chat_migration)'` → exit 0、8 passed。
- `cargo nextest run -p task-core` → exit 0、1120 passed。
- `bash scripts/dev/test-affected.sh` → exit 0、6688 passed・21 skipped。web は tsc -b 通過、vitest 100 files・690 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::` と `corepack pnpm@12.6.0 -C web gen:types` で
  `docs/api/v1/api-v1.schema.json`・`web/api/generated/*` を再生成した。

## 未解決事項

- 添付の upload（`AttachmentStore::upload`）は task-core で thread 種別を見ない。review thread への
  `POST …/attachments` の 403 は api-op の葉で API 側に置く。
- `cos_review_report_tx` は呼び出した run が review thread の run かを検査しない（`run_id` を provenance に記録するだけ）。
  403 の判定は api-op の葉が CoS run の credential で行う。
- review thread の queue が止まった（`queue_paused`）とき、`resume-queue` は 403 なので人は戻せない。D4 の
  `skipped_overlap` が続かないよう、dispatch の葉で止まった queue の扱い（sink が解く等）を決める必要がある。

## 提案

- 上の 3 点目を dispatch の葉の受け入れ条件に入れる。
