---
unit: metering
task: 01M51X7MZPYQXVWV2WDG2N70C1
title: metering — 全 run の消費量の計測
tasks: [01M51X7MZPYQXVWV2WDG2N70C1]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# metering — 全 run の消費量の計測

## 実装済み
- migration 0086（統合時に 0085 から振り直し）: `decision_log_usage.attribution_rule` / `shared_overhead`、`decision_log_campaign_activity`。
- `DecisionLogUsage::{cos_turn, jev_call}`、`CampaignMetering`（直接帰属 / 共有オーバーヘッド / 規則別 / elapsed / run 時間総和）、`SqliteStore::decision_log_campaign_metering` / `decision_log_campaign_touch`。
- Jev 費用: `ClassificationEntry.cost_usd` → `Dispatcher::record_jev_usage`（lane_assist）。`classify_stage` が Jev 応答の費用を載せる。
- `write_decision_log` が campaign の最後の活動を記録（elapsed の終端）。
- ADR 2026-10-16-metering-attribution。

## export への出力（export-join.md で実装済み・mock 検証済み）
- `attribution_rule` / `shared_overhead` / `elapsed_ms` / `run_wall_sum_ms` は export の summary に別欄で出る。total・by_role は直接帰属のみで、共有オーバーヘッド（CoS）は `shared_overhead` に分かれる。Jev 費用は `task_level_consumption` に出る。
- 空 export・必須欄の欠測を失敗にする接続確認 `campaign export --verify-connected` も済み（`crates/celerisctl/tests/campaign_connected.rs`）。実 campaign での実経路確認は未実施（export-join.md の手順）。

## mock 検証済み（temp DB / in-memory、注入時計）
- `every_run_kind_has_a_usage_row_and_shared_overhead_is_separate`: planner・worker・再試行・reviewer・Jev・CoS の行、CoS は共有オーバーヘッドで別集計、実支出は欠測のまま。
- `reprocessing_the_same_events_does_not_double_count`: 同じ event の再処理で二重計上しない（4 回再処理しても行数・合計が不変）。
- `missing_usage_stays_null_and_is_distinct_from_zero`: 欠測は 0 にせず NULL のまま 0 と区別し、n_missing に出る。
- `elapsed_and_run_time_sum_are_separate_fields`: elapsed 90s、run 総和 120s（並列）、古い時刻の再処理で縮まない。
- `lane_assist::jev_classification_cost_is_metered_once_per_campaign`: Jev 費用あり/なし、2 tick で 1 行。

## 実経路検証済み
- なし。

## 未実施
- CoS chat の実 turn から `cos_turn` を呼ぶ配線（CoS の turn 単位の usage が repo に event として存在しない。呼び出し口と規則は用意済み）。
- Jev の token 数（応答に無い）。Jev 再分類の `call_attempt` は常に 1（同じ subject の再分類は置換）。
- `decision_log_interventions` の書き手（identity.md から持ち越し、未着手）。

## 提案
- なし（summary への `CampaignMetering` 反映は export-join で済み）。
