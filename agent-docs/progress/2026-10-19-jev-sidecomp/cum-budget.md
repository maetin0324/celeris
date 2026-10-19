---
title: "hash 変更でも既消費を承認総枠へ合算（B1/C1 累計予算）"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: cum-budget
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# cum-budget

- 穴: 台帳 key と out dir が manifest hash 単位のため、hash を変えた再 manifest では既消費が見えない。
- 修正: `budget::series_prior_spend`（side manifest のみ。out の親 dir の兄弟で run-meta の campaign_id が同じ・hash が違うものの確定消費+予約を既存 `spend` で合算。読み取り専用 `Ledger::open_readonly`）。`runner::run` は out 作成前に、既消費が承認総枠（budget.max_tokens/max_wall_secs）に達していれば拒否し、非同期投入 gate は `Spend::plus_prior` で残枠を判定する。主比較（非 side）は常に 0 で計上不変。
- 試験: `crates/campaign/tests/side_budget.rs` 4 件（hash 変更後も合算／系列違いは合算しない／総枠到達で実行前拒否・out 未作成／残枠が予約未満なら投入 0／主比較は不変）。
- 証拠: `cargo test -p celeris-campaign` 全 pass（既存 final_accounting 等は無変更）、clippy -D warnings pass、fmt 済み。
- 未解決（後続 WU repair-evidence-budget で解消: 兄弟走査は廃止し、測定 ID ごとの固定系列台帳へ置換）: 系列の同定は同じ親 dir の兄弟 out に限った。final_accounting.rs 側の最終集計への合算は未実装（実行前の予算 gate のみ）。
