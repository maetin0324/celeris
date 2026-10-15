---
title: "export-agg: campaign 単位の消費集計（task-core）"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# export-agg（unit）

ADR `agent-docs/adr/2026-10-15-decision-log-export.md` D5〜D8 の集計・export を `crates/task-core/src/decision_log_export.rs` に純関数で入れた（`build_campaign_export`）。store には読み出しのみの `decision_log_export_campaign` を足した。CLI・配線は後続 unit。

## 仕様の要点

- 出力: `records.jsonl`（record_type = decision/run/outcome/usage/intervention、各行に schema と `dedup_key`）、`summary.json`（`CampaignSummary`）。
- 集計欄は 4 つ組 `Stat{sum,n_observed,n_missing,n_estimated}`。観測 0 件の和は null。定価換算は常に estimated、実支出は定価で代用しない。
- by_role（cos/planner/worker/reviewer/jev を常に出す）、by_retry（別表）、by_arm、total、cluster（wait/run/node_hours/queue、wall_ms と別）、runs_by_end（失敗・timeout・cancel を分母に残す）、wu_verdicts / campaign_verdicts（別表）、interventions_by_kind（4 区分）、exec_vs_proposed、incomplete、excluded_rows。
- 別 campaign の行は除外（campaign 未記載の消費・介入は run/decision/task で結べるものだけ採る）。run_id・dedup_key・intervention_id で 1 件に潰し、合算しない。
- `classifier_confidence` と `propensity` は別欄（propensity 無しは null）。

## 証拠

- `cargo test -p task-core --lib decision_log`: 13 passed（export 6 本: 別 campaign 混入なし、再処理で二重計上なし・byte 一致、欠測 null / 推定フラグ / 区分分離、失敗を分母に残す・別ラベル・誤帰属なし、retry/role 別表と Jev、confidence≠propensity と介入区分）。
- `bash scripts/dev/test-parallel.sh`: 5161 passed, 0 failed。
- `cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決事項

- quota は消費行に載らないため summary の `quota` は null、`incomplete` に常に `quota`。取得元の配線は log-wire。
- `export_cursor` は呼び出し側が渡す（入力行に recorded_at を持たないため）。export-cli が最大 seq を渡す。

## 提案

- 介入区分名は `research_judgment`（log-core の決定）に ADR 表記を揃える。
