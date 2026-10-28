---
title: task-dispatch の cron sink を review thread へ、review の CoS run の文脈と session
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# task-dispatch の cron sink を review thread へ、review の CoS run の文脈と session

完了日: 2026-10-28

ADR [2026-10-28-cos-review-back-thread](../../adr/2026-10-28-cos-review-back-thread.md) の D2・D3'・D5' の task-dispatch 部分と、
core-thread の未解決事項（止まった review queue）の扱い。

## 変更

- `crates/task-dispatch/src/dispatcher/cos_chat/review.rs`
  - `CosReviewSink` は `cos_review_thread_open` で review thread を開き、`cos_review_input_post` で入力を積む。統括 thread には
    何も積まない。`client_message_id` の照合（D3）・待ち行列の判定（D4 の `skipped_overlap`）・CoS chat 不在の `error` は
    review thread で行う。統括 thread の人の発言は見直しを止めない。
  - `COS_REVIEW_INSTRUCTION` を D5' の文に直した（伝えるべきことがあるときだけ `chat.review_report` で 1 件報告、無ければ
    review thread に「変化なし」と 1 行）。
  - 止まった queue（`queue_paused`）の扱い（決定）: 人が review run を止めると queue が止まり、review thread の
    `resume-queue` は 403 なので誰も戻せない。sink は止まった queue を見たら、待っている入力を取り消し
    （`chat_message_cancel`）、新しい入力で queue を再開する。新しい入力の期間は、取り消した入力の分も含むよう、
    run が取った最後の入力の予定時刻（key `cron-cos-review:<job>:<unix 秒>` から読む）から始める。待つ入力が無ければ
    そのまま積んで再開する。こうして `skipped_overlap` が続かない。
- `crates/task-core/src/chat/review.rs`: `cos_review_input_post` が `queue_paused` を解く（`resume_queue: true`）。積むのは
  sink だけなので、新しい周期の入力は必ず走らせる。task-core の変更はこの 1 行と doc comment だけ。
- `crates/task-dispatch/src/dispatcher/cos_chat/launch.rs`: thread 種別から文脈の範囲を返す `cos_context_scope` を足し、
  `Review` を `General` と同じ `All`（全案件の要約・全 thread の未解決の受信箱項目）にした。skill も同じ
  （`cos-operator`・`cos-inbox-triage`）。session は thread ごとの照合 key（`thread_id` を含む）で別になる。
- 試験 `crates/task-dispatch/src/dispatcher/tests/cron_cos_review.rs`
  - 既存 2 件を review thread に向け、統括 thread の message が 0 件であることを足した（周期ごとに 1 件・同じ周期の再 tick と
    再起動で増えない・待ち行列が残る間は `skipped_overlap`・CoS chat 不在は `error`）。
  - `cos_review_is_not_held_back_by_the_persons_queued_message_in_the_general_thread`: 統括 thread の人の待ち入力では
    `skipped_overlap` にならない。
  - `cos_review_queues_the_next_period_after_a_person_stopped_a_review_run`: run を止めて queue が止まっても次の周期の
    入力が積まれ、待っていた入力は取り消され、期間は止めた run の入力から、queue は再開。待つ入力が無い停止でも同じ。
  - `cos_review_thread_runs_get_the_general_cos_context_scope` と
    `cos_review_run_has_the_general_cos_context_and_its_own_session`: fake harness（shell script、LLM なし）で review run を
    起こし、request.json に全案件の要約と統括と同じ skill が入ること、run の session が review thread のもので統括 thread
    の message が 0 件であることを確かめる。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `cargo nextest run -p task-dispatch -p task-core -E 'test(cos_review) | test(cos_chat) | test(cron)'` → exit 0、187 passed。
- `bash scripts/dev/test-affected.sh` → exit 0、6692 passed・21 skipped。web は vitest 100 files・690 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は task-dispatch の 3 file、task-core の `chat/review.rs`、この進捗ファイル。

## 未解決事項

- 受信箱項目が review run の文脈に入ることは `cos_context_scope` の単体試験（`General` と同じ `All`）で確かめた。実際に
  受信箱項目を積んだ結合試験は置いていない（項目を積むと統括 thread で triage run が起き、試験の主題から外れる）。
- 報告する／しない（`chat.review_report` で統括 thread に 0 件・1 件）の試験は api-op の葉の担当。

## 提案

- なし。
