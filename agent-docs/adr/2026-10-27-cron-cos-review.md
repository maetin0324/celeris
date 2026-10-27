# ADR 2026-10-27: cron から統括 CoS の thread へ定期見直しの入力を積む

- 日付: 2026-10-27
- 状態: 採用・実装済み
- task: 01M5XXF7QJST7W1VWK0C5XT128
- 関連: [ADR-0131](0131-cron-jobs.md)（cron job）、[ADR 2026-10-27-project-brief](2026-10-27-project-brief.md)（案件の要約）、
  [ADR 2026-10-27-project-stagnation-inbox](2026-10-27-project-stagnation-inbox.md)（停滞の検出）、
  [ADR 2026-10-15-root-done-cos-summary](2026-10-15-root-done-cos-summary.md)（daemon が CoS の thread へ入力を積む先例）、
  [ADR 2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)（統括 CoS の thread）

## 背景

CoS が起きるきっかけは、人の発言・受信箱の新着（triage）・root task の完了の 3 つだけだった。定期の見直しが無いので、
人の操作待ちが何時間続いているか、案件をまたぐ停滞・優先・資源を CoS が見に行く起動が無く、外部 operator の巡回
ループ（60 秒ごとの受信箱の polling）が代わりをしていた。cron（ADR-0131）は task を作るだけだった。

## 決定

- D1（雛形の種類）: cron の雛形に `extra.kind` を足す。無ければ従来どおり task を作る。`"cos_review"` は task を
  作らず、発火ごとに統括 CoS の thread へ入力を 1 件積む。それ以外の値は 422。`cos_review` の雛形は task の欄
  （`acceptance`・`assignee`・`harness`・`lane`・`project`・`repos`・`priority`・`extra.mode`・`extra.action`）を
  書けない。`overlap` は `skip` だけ（溜めた入力は CoS の待ち行列にあるので `queue` は持たない）。`title` は入力の
  見出し、`objective` は CoS への指示（空なら既定の指示）。型は `task_ops::cron_jobs::{CronJobKind, template_kind}`。
- D2（dispatcher は積むだけ）: 発火の規則（予定時刻・取りこぼし・`catch_up`）は task の job と同じ `fire_job` を通る。
  入力を積むのは dispatcher が渡す `CronInputSink`（`dispatcher/cos_chat/review.rs` の `CosReviewSink`）で、統括 CoS の
  thread に `mode = queue` の入力を 1 件 post するだけ。CoS の run は通常の待ち行列の claim が起こす。dispatcher も
  task-ops も LLM を呼ばない。入力の本文は store と dispatcher の判定から決定的に組む:
  - 前回の見直し（この job の最後の `created` の予定時刻）より後に `updated_at` が進んだ案件の要約（ADR
    2026-10-27-project-brief。アーカイブ済みは除く。初回は全件）。題・id・更新時刻・書いた者・現状（200 字）・待ち。
  - いま出ている停滞（`DaemonSnapshot.stagnation` と同じ `StagnationFinding`）。
  - 各一覧は 20 件まで（残りは件数だけ）。CoS は必要な案件だけ `GET /api/v1/projects/{id}` で詳しく読む。
- D3（重複しない）: 入力の `client_message_id` は `cron-cos-review:<job id>:<予定時刻の unix 秒>`。sink は post の前に
  同じ key の message を探し、あれば積まずに `created`（detail に既存の message）とする。積んでから履歴と
  `next_fire_at` を書くので、間で落ちても再起動後の同じ予定時刻の発火は同じ入力に束なる。履歴の UNIQUE
  （job・予定時刻・きっかけ）と `next_fire_at` の更新が同じ予定時刻の 2 回目の発火を止める。
- D4（溜めない）: 統括 CoS の thread に待ち行列の入力（人の発言を含む）が残っていれば積まずに `skipped_overlap`。
  CoS chat が無い・無効・使えないときは積まずに `error`（detail に理由）。どちらも次の予定時刻に再び評価する。
  `previous` は `created` の行だけから求めるので、飛ばした回の変化は次の入力に入る。
- D5（手動実行しない）: `POST /cron-jobs/{id}/run` は `cos_review` の job を監査の記録を始める前に 422 で拒否する
  （sink は daemon の tick にしか無い）。`run_now` も同じ検査（`check_manual_run`）を通る。
- 履歴の `outcome` は既存の値を使う（積んだ = `created`、`task_id` は無し）。`cron_job_runs` の CHECK 制約は変えない。

## 結果

- 定期の見直しは job を 1 件足すだけで有効になる（例: `0 * * * *`、`template = {"title": "定期見直し", "kind":
  "cos_review"}`）。人の操作待ちの長さは受信箱の `human_wait_aging` が CoS triage に、案件の停滞と要約の変化はこの
  入力が統括 CoS に届ける。
- 試験: `crates/task-ops/src/cron_jobs/tests.rs`（検証・sink 経由の発火・sink なしの `error`・手動実行の拒否）、
  `crates/task-dispatch/src/dispatcher/tests/cron_cos_review.rs`（時計を差し替えて周期ごとに 1 件、同じ周期の再 tick・
  post 後の落ちを模した再起動・素の再起動で増えない、待ち行列が残る間は `skipped_overlap`、CoS run を起こさない）、
  `crates/task-api/tests/cron_jobs.rs`（作成と手動実行の拒否）。
