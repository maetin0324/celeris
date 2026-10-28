---
title: 定期見直し（cron の cos_review）を裏の thread で回し、変化があるときだけ統括 CoS チャットに報告する
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# 定期見直し（cron の cos_review）を裏の thread で回し、変化があるときだけ統括 CoS チャットに報告する

完了日: 2026-10-28

人の依頼（2026-10-28、統括 CoS チャット seq 3936 の要旨）: 定期見直しの入力と「変化 0」の返事が毎回統括 CoS チャットに出て
邪魔。裏の thread で監視し、何かあったときだけ統括 CoS チャットに報告する。

設計は ADR [2026-10-28-cos-review-back-thread](../adr/2026-10-28-cos-review-back-thread.md)（状態: 採用・実装済み）。
旧 ADR [2026-10-27-cron-cos-review](../adr/2026-10-27-cron-cos-review.md) の D2 を置き換え、D1・D3〜D5 は保つ。
各葉の詳細は [2026-10-28-cos-review-back-thread/](2026-10-28-cos-review-back-thread/) の葉ごとのファイル。

## 変更（葉ごとの commit 件名）

- adr — `adr: 定期見直しを統括 CoS の裏 thread で回し、変化があるときだけ報告する`。
  thread 種別 `review`、報告 operation `chat.review_report`、`GET /api/v1/cos/review-thread` などの名前を決めた。
- core-thread（[core-thread.md](2026-10-28-cos-review-back-thread/core-thread.md)）—
  `phase 1: task-core に統括 CoS の裏 thread（review）と定期見直しの報告の書き込み関数を足す`、
  `phase 1: core-thread の進捗に範囲と再実行の結果を足す`。`ChatThreadKind::Review`、migration 0101
  （CHECK と部分 UNIQUE index だけ。`chat_messages` に触らない）、`cos_review_thread_open`・`cos_review_input_post`・
  `cos_review_report_tx`、一覧・検索・未読から review を外す、投稿経路の 403 `chat_thread_read_only`。
- api-op（[api-op.md](2026-10-28-cos-review-back-thread/api-op.md)）—
  `phase api-op: task-api に chat.review_report と review thread の読み取りを足す`、
  `phase api-op: 進捗に test-affected の結果を書く`。`POST /api/v1/cos/review-report`（ALLOWED に登録）、
  `GET /api/v1/cos/review-thread`、添付の 403、`CronJobView.review_thread_id`。
- dispatch（[dispatch.md](2026-10-28-cos-review-back-thread/dispatch.md)）—
  `phase 1: cos_review の入力を review thread に積み、review run に統括 CoS の文脈を渡す`。`CosReviewSink` の積み先を
  review thread に変え、D3〜D5 の判定も review thread で行う。`COS_REVIEW_INSTRUCTION` を ADR D5' の文にした。
  止まった review queue は次の周期に sink が解く。
- skills（[skills.md](2026-10-28-cos-review-back-thread/skills.md)）—
  `phase skills: CoS skill に定期見直しの review thread と chat.review_report の扱いを書く`。
  `cos-operator` と `cos-inbox-triage` に定期見直しの run の節を足した。
- web（[web.md](2026-10-28-cos-review-back-thread/web.md)）—
  `phase web: cron の画面から review thread の履歴を読み取り専用で開く`、`phase web: 進捗に試験と検査の結果を書く`。
  cron の画面の `cos_review` job から review thread を読み取り専用で開く。ホームの thread から review を除く。
- close（この葉）— `phase close: 進捗ファイルを閉じ、ADR を実装済みにする`。この進捗ファイルと ADR の状態・付記。
  実装は変えていない。

## Task の受け入れ条件と確かめた試験

- 発火で統括 thread に入力も返事も積まれず、review thread で CoS run が走る —
  `crates/task-dispatch/src/dispatcher/tests/cron_cos_review.rs`（統括 thread の message 0 件、review thread に 1 件）、
  `cos_review_run_has_the_general_cos_context_and_its_own_session`（fake harness で review run を起こし、統括 thread は 0 件）。
- 報告すべき事があるときだけ統括 thread に 1 件、変化 0 では何も出ない —
  報告 1 件は `crates/task-api/tests/cos_operations_review_report.rs`（review run の `chat.review_report` で統括 thread に
  assistant 1 件・監査 1 行、再送で増えない、統括 run は起きない）。報告しない回は上の dispatch の試験（run が
  `chat.review_report` を呼ばなければ統括 thread は 0 件）。CoS が報告するかの判断は LLM の run が行うので、実機での
  確認は下の「本番への移行」の手順で人が行う。
- 旧 ADR の D3〜D5 — `cron_cos_review.rs` の既存の試験（同じ周期の再 tick・再起動で増えない、`skipped_overlap`、CoS chat
  不在の `error`）と `crates/task-api/tests/cron_jobs.rs`（手動実行の 422）。`crates/task-ops/src/cron_jobs/tests.rs` は
  変えていない。
- ADR と skill — ADR に D1〜D8 と付記、skill は skills の葉。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。agent-docs だけの変更で、対象の crate と web は無い（`--list` で確認）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 各葉の試験の結果は葉ごとの進捗ファイルにある（葉の提出時の HEAD で実行したもの）。

## 本番への移行

- config の変更は無い。cron job の雛形（`template.kind = "cos_review"`、schedule・timezone・`template.objective`）も
  変わらない。既存の job をそのまま使う。
- migration 0101 は release の起動時に走る。`chat_threads` を作り直して CHECK と index を足すだけで、`chat_messages` には
  触らない。統括 thread に溜まった過去の定期見直しの message は残る（ADR D7）。
- CoS skill を本番 KB に取り込む（人が行う）:
  `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage`。

release の後、人が次の手順で確かめる。

1. cron job の id を調べる: 画面の「定期実行」で `cos_review` の job を開く。celerisctl なら `celerisctl cron list` の
   出力から `template.kind` が `cos_review` の job の `id` を読む。
2. 次の発火の後に履歴を見る: `celerisctl cron history <job id> --limit 5`。release 後の最初の行が `outcome = created`
   であることを確かめる。`skipped_overlap` が続くなら、review thread に待ちの入力が残っている（手順 4 で見る）。
3. 統括 CoS チャットに見直しの入力が出ないことを確かめる: 画面のホームの統括 CoS チャットに、発火の時刻の
   「定期見直し」の入力と返事が増えていないことを見る。報告すべき事があった回だけ、CoS の message が 1 件出る。
4. review thread に積まれたことを確かめる: `celerisctl cron show <job id>` の `review_thread_id` が null でないこと。
   画面の「定期実行」の job の詳細で「見直しの記録（読み取り専用）」を開き、発火の時刻の入力と CoS の返事
   （「変化なし」または報告した旨）があることを見る。API なら `GET /api/v1/cos/review-thread` と
   `GET /api/v1/chat/threads/<review_thread_id>/messages`。
5. 統括 CoS チャットの一覧に review thread（題『統括 CoS（定期見直し）』）が出ないことを見る。

## 未解決事項

- 統括 thread の報告の message から review thread へ移る link（ADR D6 の 3 点目）は web に無い。provenance は message に入っている。
- `cos_review` の job の「手動実行」ボタンは画面に残る（API が 422 で拒否し、結果欄に出る）。
- review thread 宛ての `POST …/forwards` は 422 `validation` で、ADR D1 の 403 `chat_thread_read_only` と code が違う
  （書き込みは起きない。ADR の付記に記録した）。
- CoS が実際に「報告する／しない」を正しく選ぶかは LLM の run の挙動で、試験では確かめていない。本番の手順 3・4 で確かめる。
- 本番 KB への skill の取り込みは人が行う。

## 提案

- web: 統括 thread の message の provenance が `cos_review_report` のとき、`/cron/<job id>#review-thread` へ移る link を出す。
- web: `cos_review` の job では「手動実行」ボタンを出さない。
- task-core: `chat_relay_forward_tx` で宛先が review thread のときの code を 403 `chat_thread_read_only` に揃える。
