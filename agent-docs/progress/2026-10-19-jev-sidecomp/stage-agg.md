---
title: "Jev 段階別集計（campaign crate）"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: stage-agg
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# stage-agg

`crates/campaign/src/jev_stages.rs`（純関数 `aggregate` / `run_json`）と CLI `jev-stages` を追加。判定は既存 `jev_smoke::judge` を再利用。本体の記録欄は変えていない（旧欄に影響なし）。

## 呼び出し形
`cargo run -p celeris-campaign -- jev-stages --records <json>`（manifest 不要）。試験: `cargo test -p celeris-campaign --test jev_stages`。

入力: 配列（または `{"records":[...]}`）。各要素 `{arm, task_id, event(routing_decided), allowed?, max_lane?, version?, fixed_lane?, jev_usage:[{cost_usd|null}], run:{proposed_model, exec_model, fallback_reason?, stopped_by?:"budget"}, outcome:{result, retries, cost_usd}}`。出力: `total` と `by_arm` の段階別件数（分類試行・jev_ok・lane 提案変更・制約適用後 lane・model 変更・抑止理由別・成果/再試行/費用）。

## 方針
- 変化 0・効果なしは 0 のまま出す（欄は常に出力）。欠測は `*_missing` / `model_unknown` に分け、推測で埋めない。
- `confidence` は読まない（試験 `confidence_is_ignored`）。propensity 扱いなし。

## 証拠
- `cargo test -p celeris-campaign --test jev_stages --test jev_smoke`: jev_stages 5 passed、jev_smoke 7 passed。
- `bash scripts/dev/test-parallel.sh` exit 0（5821 passed, 0 failed）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決・提案
- 本体の RD に `fixed_lane`/`jev_lane`/Jev 版が無い（inventory 不足 1）。入力の `version`/`fixed_lane` は呼び出し側が渡す。記録から入力 JSON を組む抽出は synth-e2e 側で扱う。
