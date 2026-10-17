---
title: "P1-6 record-contract: 正式 export と外側 run の結合入力を固定"
tasks: [01M54J975KSQ3K2DYTWEAH2A0H]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# record-contract

既存 writer・DB schema は変えず、`build_campaign_export`（`crates/task-core/src/decision_log_export.rs`）の出力契約だけを固定した。

- usage record: `run_id` / `dedup_key` の生値、`usage_key`（kind=run|call）、`usage_state`（observed / missing / shared_unattributable）、`run_known`。
- trial record: `consumption_state`（observed / missing=行はあるが全欄欠測 / unmetered=走ったが行なし / not_executed）。
- summary `integrity`: 一致した重複数、競合した鍵（先に来た行を残す）、manifest hash 不一致で除いた run・消費行数、run 行が無い消費行数（遅延観測として record に残し試行には結合しない）。
- `verify_connected` は競合重複を問題として報告する。
- `CampaignSummary` は API schema に出ないので GUI/web の型再生成は不要。

## 証拠
- `cargo test -p task-core decision_log` 16 passed / `cargo test -p celerisctl --test campaign_export` 1 passed / `--test campaign_connected` 5 passed / `cargo test -p task-api --test campaign_ident` 1 passed
- `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0
- `test-parallel.sh` 全体は未実行（変更は export の純関数と試験のみ）。

## 提案
- 競合重複は現在「先着を残す」。store は dedup_key 主キーなので実 DB では起きず、外部入力用の防御。
