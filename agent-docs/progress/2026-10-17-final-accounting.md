---
title: "campaign 最終 export へ重複排除済み消費と欠測を接続"
tasks: [01M54KCEXBRKXSHCWYZX4465YS]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# final-accounting

`crates/campaign/src/final_accounting.rs`（ADR `2026-10-17-final-accounting.md`）。既存 writer は再実装せず、record-contract の `records.jsonl` を読む。`export::export_with`・`report::check_consumption_claims`・CLI `final-accounting` / `export --records` / `check-report --records` に接続。budget 制御は未変更。

## 証拠
- `cargo test -p celeris-campaign --test final_accounting` 11 passed。
- `cargo test -p celeris-campaign` 250 passed / 0 failed、`cargo fmt --all -- --check` exit 0、`cargo clippy --workspace --all-targets -- -D warnings` exit 0。

## 未解決・提案
- `tokens` 指標は Jev call が token を持たない（`jev_call` は list_cost のみ）限り、Jev を使う C 腕を含む campaign では完全にならず削減は確定しない（意図した厳しさ）。Jev の token 計上を writer 側で足すかは別 task の判断。
- A 腕は Celeris 外なので run 内訳が無く、定価換算・実支出は完全にならない（tokens は外側の値で完全にできる）。
- CoS は campaign に割れないため、呼ばなかった根拠（`--cos-not-called`）が無い限り未取得。
