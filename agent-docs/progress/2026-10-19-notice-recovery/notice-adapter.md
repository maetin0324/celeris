---
unit: notice-adapter
status: done
completed: 2026-10-19
---
# notice-adapter: 測定 adapter（有界 blocking 通知受信・永続適用後 ack・処理証跡）

- `ipc.rs`: 新 op `Receive{wait_ms, after_seq}`（push 用 outbox を最大 `wait_ms`≤5s 待つ有界 blocking receive）と
  `AckNotice{seq}`（選択的 ack）。push 用 outbox（`pushbox`）は `advance` の piggyback ack では減らない。版は 1 のまま（追加のみ）。
  `NoticeChannel`: receive / hold（登録前到着。ack せず手元に保持）/ settle（`notice_processed` を ledger に sync → ack → `notice_acked`）。
- `ext.rs`: `NoticeLedger`（独立 ledger、`role=control_consume`。外部側の `notice_sent`＝独立観測と別 file）。
  行に notice_seq / copy / logical・physical ID / wait・task・work-unit・run ID / outcome。開き直すと処理済み seq・消費済み論理 job を復元。
- `celeris_side.rs`: `enable_notices(channel, disable_poll)` / `pump_notices`。`Dispatcher::apply_cluster_job_notice`（notice-hook）へ渡す。
  poll 無効時は poller が失敗（`poll_attempts` を数える）。時計は ext の物理起動期限で進める（完了検出ではない）。
- `baseline.rs`: `set_notices`。通知主導では `advance` の返す通知も poll の `lookup` も使わず、同じ push 通知で待機を再開。復旧時の起動照会 1 回は残す。
- 試験（`dagbench_ipc.rs`）: `dagbench_notice_receive_is_bounded_and_ack_is_selective` / `..._restart_between_apply_and_ack_does_not_apply_twice` /
  `..._baseline_resumes_by_push_and_suppresses_duplicates` / `..._celeris_side_resumes_by_push_with_polling_disabled`。

## 証拠
- `cargo test -p celeris-campaign --test dagbench_ipc dagbench_notice` → 4 passed。`cargo test -p celeris-campaign` 全 binary ok（failed 0）。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決
- 適用（dispatcher が wait を閉じた）と `notice_processed` 記録の間で止まった場合、再配送は Duplicate として suppressed になり、
  その wait の `notice_continuation` 行は ledger に残らない（継続自体は store の `ClusterJobWaitFinished` が根拠）。
- 実 process 試験（`recovery_notice_continuation` / `recovery_lost_response`）は次 unit（notice-process-tests）。
