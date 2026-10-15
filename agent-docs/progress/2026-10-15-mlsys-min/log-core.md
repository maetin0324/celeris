---
title: "log-core: 判断・実行・成果のラベル欄と冪等な記録（task-core）"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# log-core（unit）

ADR `agent-docs/adr/2026-10-15-decision-log-export.md` の D1〜D7 の保存層を `crates/task-core` に入れた。配線（dispatch/api）・集計・CLI は後続 unit。

## 変更

- migration `0082_decision_log.sql`（統合時に credential_waits の 0081 と衝突したため 0082 に繰り下げ。`SCHEMA_VERSION` 82）: `decision_log_runs`（run_id 主キー）/ `decision_log_usage`（dedup_key 主キー）/ `decision_log_interventions`（intervention_id 主キー）。events は触らない。
- `decision_log.rs`（型）・`store/decision_log.rs`（書込み・読出し）。提案 / 実行 / 成果の 3 群は列が別で、群ごとの upsert は他群を上書きしない。再処理は主キー置換で合算しない。
- `RunEndLabel`（wall-clock の予算切れは `timeout`、元の値は `run_end_raw`）、`WuVerdict`（導出）、`CampaignVerdict` は別欄。未判定は NULL。
- `classifier_confidence`（0..=1 を検査）と `propensity` は別列。`DecisionLogProposal::from_routing_record` で旧 `routing_decided` から作れる（新欄は None）。
- `decision_log_runs_by_exec` は `exec_*` でのみ引く（提案先頭には帰属しない）。
- 既存の `SCHEMA_VERSION` 固定の試験（80→81）と `delivery.rs` の版数列を更新。

## 判断

- 介入の区分名: 依頼 objective に合わせ `research_judgment`（ADR 本文は `research_decision`）。後続 unit は `research_judgment` を使う。ADR 側の表記合わせは export-cli か log-verify で行う提案。
- 既存 event の struct（`RoutingRecord` など）には欄を足さず、新欄は新テーブルに置いた（log-wire が書く）。

## 証拠

- `cargo test -p task-core --lib decision_log`: 7 passed（誤帰属なし、再処理で行数・合計が増えない、confidence が propensity に入らない、旧 event 読み取り、失敗/timeout/cancel/retry の行保持、別ラベル）。
- `bash scripts/dev/test-parallel.sh`: 初回は `delivery::tests::migration_0042...` が版数列に 81 を含まず 1 件失敗 → 修正後 `cargo test -p task-core` で再確認（下記）。
- `cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項

- `cluster_*`・`quota` の取得元の配線、`cos` 消費の取得元は log-wire / export-agg。
- `list_cost_usd_estimated` は常に 1（定価換算は常に推定）。保存列だが export 側の派生でもある。

## 提案

- ADR の介入区分名を `research_judgment` に揃える。
