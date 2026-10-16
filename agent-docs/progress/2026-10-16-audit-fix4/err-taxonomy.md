---
title: 監査修正4 err-taxonomy（通信障害・cancel 失敗で追跡を失わない）
updated: 2026-10-16
status: done
tasks: [01M536XRNEV0FXK7D4SVSNKY03]
---

# 監査修正4 err-taxonomy

設計は [ADR](../../adr/2026-10-16-campaign-comm-failure-tracking.md)。完了日: 2026-10-16。

## 着手時の確認

関連タスク（実アダプタ・台帳 / pilot 修正 / 予算）の merge 済み差分を `crates/campaign` で確認: `drive_async` の
`DriveErr::Exec` 一律 error・cancel 失敗の無視（`let _ = cancel_row`）は残っていたため修正した。

## 変更

- `executor.rs`: `ErrKind`/`ExecError`、trait の lookup/submit/track/usage/poll/cancel を `ExecError` に、`stopped` を追加。
  mock に fail_next_polls/usages/cancels・unconfirmed_stops・set_usage を追加（応答喪失は `lose_next_submit_responses`）。
- `ledger.rs`: state `submit_unknown`/`cancel_pending`、欄 `started_at`/`observed_usage`/`reserved`/`comm_failures`/`last_error`。
- `runner.rs`: `drive_async` の分類処理、`cancel_row` の cancel_pending→停止確認、run 先頭の cancel 再試行。`DriveErr` 削除。
- `celeris_exec.rs`: POST 失敗=Unknown、4xx=Refused、事前照会失敗=NotSent、walk 失敗=Interrupted。`dispatch.rs`: usage/stopped 委譲。

## 証拠

- `cargo test -p celeris-campaign --no-fail-fast`: 全 pass（ledger_async 12 件、うち新規: 応答喪失で submit 1 回、submit_unknown の lookup 継続、
  poll 通信断→再読込後に同じ task、usage/poll 断で既観測 usage 保持、cancel 失敗→cancel_pending→停止確認で cancelled、NotSent 再試行、旧行読込）。
- 全体 gate の結果は報告本文に記載。

## 未解決事項・提案

- `submit_unknown` が照会で永遠に見つからない場合の人の解消手順（CLI）は未実装。watch-driver / closeout で runbook 化を提案。
- 予算の予約解除条件と想定最大超過の runbook は budget-reserve / closeout WU。

## 最終 gate（再確認）

- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5554 / failed 0）。`cargo clippy --workspace -- -D warnings` exit 0。`cargo fmt --all -- --check` exit 0。
- 未使用だった試験 helper `ledger_lines` を削除（`--all-targets` の dead_code 回避）。
