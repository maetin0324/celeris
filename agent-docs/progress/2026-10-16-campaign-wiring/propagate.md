---
task: propagate
status: done
completed: 2026-10-16
---

# propagate: 子・孫・再試行・planner/reviewer への識別継承

## 既存成果の確認（作り直さない）
- 祖先解決 `SqliteStore::task_campaign_resolve` と、`write_decision_log` での全 run（再試行・reviewer・planner）への
  `apply_campaign` は main に既にある（01M51X7MZP, commit 01800415）。再実装していない。
- identity 側の未適用 patch（campaign 識別の API 欄）は api-ident WU（75caf0b2）が POST /tasks として取り込み済み。patch 自体は破棄（同じ内容を新規実装で置換したため）。
- 既存試験 `campaign_identity_reaches_every_descendant_run_retry_and_review`（root→子→孫、再試行、reviewer）が通ることを再確認。

## 実装済み
- 食い違いの修正: 同じ run が複数回 `WorkerFinished` を出す（待機からの再開）と、決定ログ側は合算、`runs[].usage`
  （`task_ops::view::runs`）は最後の値で置換していた。外側 Executor が読む値と campaign 消費行がずれる。
  `Usage::accumulate`（task-core）を唯一の合算とし、両方が使う。欠測は 0 にしない（片方だけ観測ならそれを採る）。

## mock検証済み
- `decision_log_wire::tests::runs_usage_and_decision_log_usage_agree_per_campaign`: root/子/孫 × (失敗再試行・待機再開で 2 度終了・reviewer)
  で runs[].usage の合計 = campaign 消費行の合計、3 回再投影しても行数 9・合計不変。
- 識別は `task_campaign_set`（API 経路の api-ident 試験は campaign_ident.rs 側が HTTP 経由で保存・子の継承を確認）。

- task-ops 側の試験 `view::tests::runs_usage_accumulates_repeated_worker_finished`: 同じ run の 3 回の WorkerFinished（値・値・欠測）が合算され、欠測の欄は 0 にならない。

## 実経路検証済み
- なし（temp DB / mock のみ。本番 SQLite/KB は触っていない）。

## 承認待ち
- なし。

## 提案
- executor WU: `runs[].usage` の木合計と `decision_log_usage` の campaign 合計を突き合わせる検査を export --verify-connected に足せる。
