---
title: task-api の報告 operation chat.review_report と review thread の読み取り
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# task-api の報告 operation chat.review_report と review thread の読み取り

完了日: 2026-10-28

ADR [2026-10-28-cos-review-back-thread](../../adr/2026-10-28-cos-review-back-thread.md) の D1（API 側）・D4'・D6 の task-api 部分。

## 変更

- `phase api-op: task-api に chat.review_report と review thread の読み取りを足す` — 次の内容。
  - `crates/task-api/src/cos/review.rs`（新設）:
    - `report_op`: CoS operation `chat.review_report`。`cos_operation_apply` の transaction の中で、credential の thread が
      review thread かを確かめ（違えば 422 `cos_review_report_invalid`）、`task_core::chat::cos_review_report_tx` を呼ぶ。
      task-core の `Invalid`（入力 id が review thread の入力でない・text が空か 4000 字超）も 422 `cos_review_report_invalid`。
      拒否は rejected の行として監査に残る。target は `chat_thread` / `general`（`chat.thread_create` と同じ名前の付け方）。
      `result` は `{message, created}`（`ReviewReportResult`）。
    - `POST /api/v1/cos/review-report` の直接の呼び出しは 422: CoS run は `cos_audit_context_required`（rejected の行を残す）、
      人・operator・MCP の token は `cos_review_report_invalid`。
    - `GET /api/v1/cos/review-thread`: `ChatThreadResponse`。無ければ 404 `chat_not_found`（`cos_review_thread_get`。作らない）。
  - registry: `cos/ops/surface.rs` の `ALLOWED` に `("POST", "/api/v1/cos/review-report", "chat.review_report")`。
    `match_operation` は登録済みの `/api/v1/cos/` の行を `recursive_cos` の拒否より先に通す（他の `/cos/*` は従来どおり拒否）。
  - 添付: `chat/attachments.rs` の `upload_thread` が review thread への upload を 403 `chat_thread_read_only` にする。人の
    multipart の route は本文を読む前に、CoS の `chat.attachment_upload` は効果の前に判定する（rejected の行が残る）。
  - cron: `CronJobView.review_thread_id`（`cos_review` の job だけ review thread の id、まだ無ければ `null`。作らない）。
  - schema: `cos_review_report_request`・`cos_review_report_result` を足し、`docs/api/v1/api-v1.schema.json` と
    `web/api/generated/*` を再生成した。
  - 文書: `docs/api/v1/api.md`（改訂履歴、§2 の 227・228、§3.129 の registry 表と件数 107/143、§3.129a 新設）、
    `docs/api/cron-jobs.md`（`cos_review` と `review_thread_id`）。
  - 試験が読む表の追従（範囲外の 2 file）: `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` の surface 表に 1 行と付記
    （`cos_ops_registry_matches_adr_domain_assignment_and_exclusions`）、`config/skills/cos-operator/operations.md` の登録済み操作の表に
    1 行（`cos_operator_skill_table_matches_allowed`）。skill の本文の書き換えは skills の葉に残す。

## 試験

- `crates/task-api/tests/cos_operations_review_report.rs`（新設。ADR D8 の `cos_review_report.rs` の役。objective の
  `cos_operations*.rs` に合わせて名前を付けた）:
  - review run の報告で統括 thread に assistant 1 件・provenance・監査 1 行（applied）、同じ key の再送で行も message も増えない、
    別の key の再送は `created=false` で同じ message、統括 run は claim されない、人の message 一覧に出る。
  - 案件 thread の run・統括 thread の run・review thread の入力でない id・無い id・空の text は 422
    `cos_review_report_invalid`（rejected の行）。人の直接呼び出しは 422、人の `/cos/operations` は 403、review run の直接呼び出しは
    422 `cos_audit_context_required`。統括 thread に報告は書かれない。
  - `GET /cos/review-thread` は作る前 404（2 回呼んでも作らない）・作った後 200（kind `review`）。review thread への人の upload と
    CoS の `chat.attachment_upload` は 403 `chat_thread_read_only`（CoS は rejected の行）。
- `crates/task-api/tests/chat_api.rs` `chat_api_review_thread_is_hidden_from_the_list_and_read_only`: 一覧・検索に出ない、
  `GET /chat/threads/{id}`・`…/messages` で読める、人の `POST …/messages`・`PATCH` は 403 `chat_thread_read_only`。
- `crates/task-api/tests/cron_jobs.rs` `cos_review_job_is_created_without_task_fields_and_refuses_a_manual_run` に
  `review_thread_id`（作成・取得で `null`、入力後は id、task の job は `null`）を足した。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `cargo nextest run -p task-api --test cos_operations_review_report --test chat_api --test cron_jobs --test chat_attachments` → exit 0、39 passed。
- `cargo nextest run -p task-api`（sandbox の TMPDIR のまま）→ 736 passed・4 failed。落ちたのは browser_live_cross 3 件と
  browser_waits 1 件で、launcher・credentiald の socket の bind 待ち（深い TMPDIR で bind できない既知の事象。
  `scripts/dev/test-affected.sh` は TMPDIR を短くして走らせる）。今回の変更の試験は全て通った。
- `bash scripts/dev/test-affected.sh` → exit 0、4733 passed・20 skipped（cargo）。web は tsc -b 通過、vitest 100 files・690 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::` と `corepack pnpm@12.6.0 -C web gen:types` で再生成した。

## 未解決事項

- 葉の受け入れ条件は「review 以外の run・人からの報告は 422」、ADR D4' の括弧書きは「他の run・token は 403」と食い違う。
  受け入れ条件に従い 422 にした（ADR の本文「外れたら 422 `cos_review_report_invalid`、rejected で監査に残る」とも合う）。
- `POST …/forwards` の宛先が review thread のとき、task-core の `chat_relay_forward_tx` は 422 `validation`（宛先は案件 thread
  だけ）を返し、ADR D1 の 403 `chat_thread_read_only` ではない。書き込みは起きない。task-core の範囲なので直していない。

## 提案

- ADR D4' の「403」の括弧書きを 422 に合わせる付記を close の葉で足す。
- forwards の宛先が review のときの code を 403 `chat_thread_read_only` に揃えるかを close の葉で決める（揃えるなら task-core の
  `chat_relay_forward_tx` を 1 行直す）。
