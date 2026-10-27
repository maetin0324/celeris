---
title: cron から統括 CoS の thread へ定期見直しの入力を積む
tasks: [01M5XXF7QJST7W1VWK0C5XT128]
status: done
updated: 2026-10-27
---
# cron から統括 CoS の thread へ定期見直しの入力を積む

完了日: 2026-10-27

設計は [ADR 2026-10-27-cron-cos-review](../adr/2026-10-27-cron-cos-review.md)。

## 変更
- `cron から統括 CoS の thread へ定期見直しの入力を積む` — cron の雛形に `extra.kind = "cos_review"` を足した
  （`task_ops::cron_jobs::{CronJobKind, CronInputSink, check_manual_run}`）。dispatcher の cron 段を
  `dispatcher/cron_tick.rs` に移し、`cos_chat/review.rs` の `CosReviewSink` が統括 CoS の thread に冪等キー付きの
  入力を 1 件積む（案件の要約の変化と停滞の一覧。LLM なし）。`SqliteStore::chat_message_by_client_id` を足した。
  手動実行（`POST /cron-jobs/{id}/run`）は監査の記録の前に 422 で拒否する。
- 同じ commit で、main の取り込みで残っていた `task_ops::stagnation` の試験の `secretary_summary` 欄（project-brief で
  消えた欄）を消した（task-ops の試験がコンパイルできなかった）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-dispatch --lib cron_cos_review` → 2 passed
- `cargo test -p task-ops --lib cron_jobs` → 16 passed（新規 3）
- `cargo test -p task-api --test cron_jobs` → 8 passed（新規 1）
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は
  完了報告に書く。

## 本番への移行
- migration・config 欄の変更は無い。定期見直しを有効にするには人が job を 1 件作る（本番 DB への書き込みなので人の手順）:
  `celerisctl cron create --name cos-review --schedule '0 * * * *' --timezone Asia/Tokyo --template '{"title":"定期見直し","kind":"cos_review"}'`
  確認は `celerisctl cron history cos-review` で予定時刻ごとに `created`（または待ち行列が残っていれば `skipped_overlap`）が
  1 行ずつ増え、統括 CoS の thread に入力が 1 件ずつ入ること。周期は operator の巡回ループの置き換えとして 1 時間を勧める。

## 未解決
- 人の操作待ちの長さ（`human_wait_aging`）は受信箱の組み立てで判定され dispatcher に無いので、この入力には入れていない
  （受信箱の項目として CoS triage に届く）。

## 提案
- 定期見直しの job が動き出したら、operator の巡回ループ（受信箱の 60 秒 polling）を止める。
