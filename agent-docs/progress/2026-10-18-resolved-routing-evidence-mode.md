---
task: 01M57DXA25E8KB5FC75FN0TXY7
status: done
completed: 2026-10-18
---
# ResolvedRouting.evidence_mode 欠落の読み込み

- 型の既定: `ResolvedRouting` の Deserialize で欠落→ table id あり pinned / なし latest（ADR 2026-10-18-resolved-routing-evidence-mode-default）。
- 回帰試験: `crates/task-api/tests/legacy_routing_events.rs`（llm_route_selected・routing_decided・worker_progress・worker_finished の旧 shape → GET /llm/accounts 200 と usage、/tasks/{id}/routing と task 詳細 200）。
- events の読み直しエラーに event id・task id を出す（store/events.rs）。
## 未解決・提案
- (3) 他の型の必須欄追加の網羅監査は、`routing_resolved` を含む型が `ResolvedRouting` のみであることの確認に留まる。全 Event 型の旧 shape 一括試験は別 task で提案。
- 一覧 API の部分失敗（読めない event を除いて返す）は未実装。

## delivery-repair（release gate 差し戻し）
- release gate の cargo-test で `e2e::account_pool_scenarios http_login_flow_...` が落ちた（`logged_in` が login 直後の GET /llm/accounts で false）。GET /llm/accounts は dispatcher の tick snapshot を読むため、login 直後の 1 回読みは古いことがある（evidence_mode の変更とは無関係の既存の競合）。
- 修正: 該当の読みを `wait_until(SETTLE_WAIT, …)` で出来事待ちに変えた。
- 証拠: `bash scripts/dev/test-parallel.sh` exit 0（passed 5800 相当、failed 0）、clippy exit 0、fmt --check exit 0。
