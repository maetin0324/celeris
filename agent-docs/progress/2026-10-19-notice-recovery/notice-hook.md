---
unit: notice-hook
status: done
completed: 2026-10-19
---
# notice-hook: cluster_job_wait の通知接点

- `cluster_job_wait.rs`: poll 結果の完了判定を `apply_cluster_job_statuses` に共通化（all_finished → `finish_cluster_job_wait`、変化 → `ClusterJobWaitPolled`）。
- `Dispatcher::apply_cluster_job_notice(ClusterJobNotice{wait_id, job_id, notice_id, state, exit_status})`（`#[doc(hidden)]`、本番 API ではない）。
  冪等性は永続 wait の state と `last_status`（既存 event）だけが根拠。結果: Held（登録前）/ Applied / Finished / Duplicate / UnknownJob。
  Held 以外は永続適用済み（ack 可）。新 store・測定専用状態なし。
- 試験 `notice_wait_*`（3 件）: poll を失敗させたまま通知なしでは再開せず、通知で continuation 1 回、重複・別 wait・別 job・登録前到着。

## 証拠
- `cargo test -p task-dispatch --lib cluster_job_wait` → 8 passed。

## 提案
- notice-adapter は `apply_cluster_job_notice` の戻りが Held 以外のとき ack し、notice_id と outcome を処理証跡に記録する。
