---
title: "B1/C1 別測定 ID・manifest と共通条件 hash 固定"
tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]
unit: side-manifest
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# side-manifest

- 既存の実験セット選択機能は無く（inventory 不足 4）、最小互換拡張: `SIDE_CAMPAIGN_SCHEMA`、`manifest.rs` の `is_side`/`check_side_variant`/`side_variant_hash`、`planned_configs`・runner `plan`・checker `expected` の A 除外。ADR 2026-10-19-jev-b1c1-side-manifest。
- 試験 `crates/campaign/tests/variant_pin_b1c1.rs` に 6 件追加（正例、A0/B0 manifest sha256 固定、mode 以外の差、根拠表、Sonnet+Opus 欠落、lane pin）。
- 証拠: `cargo test -p celeris-campaign` 全 pass、`cargo clippy -p celeris-campaign --all-targets -- -D warnings` pass、fmt 済み。
- 未解決: 累計予算合算・通常実行器での受理確認は cum-budget / integrate-campaign。
