---
task: 01M59KXY8345HZEE06FVZ3FWVP
wu: arm-set-core
status: done
completed: 2026-10-19
---
# arm-set-core
- ADR agent-docs/adr/2026-10-19-experiment-sets.md、実装 crates/campaign/src/manifest.rs。
- 証拠: `cargo test -p celeris-campaign --lib manifest` 15 passed（新規 9 件）。
- 提案: 後続 WU は `active_arms()`/`compare_pairs()`/`planned_configs()`/`plan_identity()` を使う。
  直書きの `secondary.subset`・`primary` 参照（checker.rs/dryrun.rs/runner.rs）は有効計画へ置換する。
- 他の試験の CampaignManifest 直書きに `experiment_sets`/`selected_sets` を足した。
