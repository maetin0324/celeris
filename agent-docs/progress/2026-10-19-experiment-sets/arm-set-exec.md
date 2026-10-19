---
title: arm-set-exec — runner/checker/export/dry-run/集計を有効計画に揃える
status: done
completed: 2026-10-19
tasks: [01M59KXY8345HZEE06FVZ3FWVP]
---
# arm-set-exec

## 実施
- 有効計画（`active_arms`/`planned_configs`/`compare_pairs`）を runner・checker・aggregate・dryrun・report・final_accounting・usage に適用。
- checker に実 run 照合（B0 全役割 Sonnet / B1/C1 共通候補）を追加。run-meta に `plan_identity`、resume 拒否。
- 全体台帳 `global_ledger.rs` を runner（開始前拒否・投入ゲート・各試行後の上書き記録）と CLI `--global-ledger` に接続。
- ADR 付記 D7–D10。試験 `crates/campaign/tests/experiment_sets_exec.rs`（5 件）。

## 証拠
- `cargo test -p celeris-campaign --test experiment_sets_exec`: 5 passed。
- `bash scripts/dev/test-parallel.sh`: 5828 passed / 1 failed（`launcher::matching_environment_runs_task_api_watch...`、単独実行は pass・約 30 秒。負荷時の時間依存と見ている）。

## 未解決・提案
- arm-set-prep（preflight/lock/env-lock/probe）は未接続: 未使用 arm の credential/probe 要求 0 は相方の範囲。
- 全体 trials 上限は manifest に欄が無く記録のみ。必要なら manifest に欄を足す提案。
- launcher 試験の flake は別途調査が要る。
