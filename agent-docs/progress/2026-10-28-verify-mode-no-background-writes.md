---
title: verify mode の daemon が cluster 待ちを poll し staging DB の task 状態を書き換える不具合を直す
tasks: [01M627ZZQRFGJHA8SE8X2M3F6H]
status: done
updated: 2026-10-28
---
# verify mode の daemon が cluster 待ちを poll し staging DB の task 状態を書き換える不具合を直す

完了日: 2026-10-28

## 原因
release e676a115a5a0 の verify（2026-10-28 18:34）で、`--mode verify` の新版が `Dispatcher::tick` の
`poll_cluster_job_waits`（ADR-0090）を回し、staging DB の wait について実の Sirius に `qstat` を打ち、job 48069 の終了を
見て BenchFS task 01M5XDTAQQ19D22E1N1TXP5QAD を Ready に戻した。検査 2 の集計の後・検査 5（n-1-compat）の集計の前に
起きたため、件数は一致し tasks_digest だけが食い違った。verify の目印（`set_eligible_tasks`、ADR-0041 D5）で止めて
いたのは cluster の liveness・master・tunnel・workspace prune・repo target sweep・lease の回収などだけで、tick の
他の裏方は止めていなかった。

## 変更
- `Dispatcher::verify_smoke_only()`（`self.eligible.is_some()`）を足し、verify では次の tick の段を何もしないようにした。
  - `poll_cluster_job_waits`: poll（クラスタへの問い合わせ）、終わった wait からの task の再開、上限超過の質問待ち化をしない。
  - staging DB の既存 task・記録を書き換える段: `reconcile_tree_units`（unit を閉じる・子 task を作る）、
    `check_liveness_and_stagnation`（`StallDetected`・障害通知）、`withdraw_stale_approvals`（承認を閉じる）、
    `task_ops::browser::expire_due`（browser の wait の期限切れで task を遷移）、`tick_credential_waits`
    （credential 待ちの再開・却下）、`reconcile_terminal_records`（runs 行・統合依頼を閉じる）、`tick_cron_jobs`
    （task・CoS 入力の作成）、`resume_browser_prereq_blocked`（Blocked → Ready）、`sync_notice_feed`（通知 feed）、
    `tick_disk_watch`（disk 監視の通知）。
  - 本番と共有の場所に staging DB の状態で手を出す段: `scratch_gc`（緊急 GC を含む）、`seed_housekeeping`、
    `cleanup_work_unit_build_caches`。
- 試験を足した（すべて偽のアダプタ・偽の poll）:
  - `cluster_job_wait::verify_mode_neither_polls_nor_resumes_nor_times_out_a_cluster_job_wait` — 問い合わせれば
    「終わった」と返る wait を、poll_secs 超過・上限超過の両時点で tick しても poll 0 回、task は Blocked のまま、
    event・wait 行（state・last_polled_at）も変わらない。修正を外すと落ちることを確かめた。
  - `cron_jobs::verify_mode_does_not_fire_cron_jobs`
  - `browser_ledger_gate::verify_mode_does_not_resume_browser_prerequisite_blocked_tasks`

## 調べて止めなくてよいと判断した段
- `settle_awaiting_children`・`abort_stale_runs`・`reap_lost_reviews`・`cleanup_cancelled_worktrees`: この instance が
  手元に持つ run・review・worktree だけを扱う（verify では smoke の分だけ）。
- `reclaim_expired_leases`・`take_over_handed_off_checks`・`reconcile_parallel_tasks`・`reconcile_ownerless_runs`・
  `recover_reviews`・`dispatch_ready`: 既に `is_eligible` で smoke 以外を飛ばしている。
- `tick_cos_chat_launch`: `disable_cos_chat_for_verify` で止まっている。daemon 側（`tick_loop.rs`）の裏方は
  `InstanceRole::Active` のときだけ動き、checkpoint・backup・chat GC・KB index も `run.rs` で verify を除いている。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-dispatch` → exit 0（lib 1055 passed、doc 7 passed）
- 結果は下の検査の節に追記する。

## 未解決
- なし。本番の daemon・systemd・config には触れていない。

## 提案
- verify の目印が `eligible.is_some()` という間接的な形のため、tick に段を足すたびに止め忘れが起きうる。
  `Dispatcher` に明示の `verify` 状態を持たせ、tick の段を「smoke に要る段」と「裏方」に分けて裏方を一括で止める
  構造にすると、今回のような漏れを防げる。
