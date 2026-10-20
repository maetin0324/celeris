---
title: 測定 launcher を研究評価の現行契約に接続する
tasks: [01M5BT8CM2EAKEG7CH9DXTS0RK]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 測定 launcher の研究評価接続

## 着手時の main（cfae4a9a）での所見の確認
- `--global-ledger` の欠落: 残っていた → `GLOBAL_LEDGER` を追加。
- `A_ACCOUNT` 無条件必須: 残っていた → 必須から外し、設定時のみ渡す（要否は CLI）。
- `--variant-b0-sha256`・`--initial-trees`: 残っていた → `VARIANT_B0_SHA256`・`INITIAL_TREES`。
- 評価側観測の収集段階: `collect-observed` は手動 CLI のみ。trial identity・台帳 job ID への結び付けは無かった → `collect-observed-out`、`grade-out` の照合、launcher `collect`。ADR `2026-10-20-launcher-research-eval-wiring`。

## 無 LLM 検証（標準 launcher 入口、`crates/campaign/tests/launcher.rs`）
- `shared_approval_runs_only_with_the_declared_global_ledger`: 省略・別 path は外部実行 0 件で拒否、宣言どおりで台帳が閉じる。
- `b_c_only_needs_no_a_credentials_but_an_active_a_arm_does`: B/C のみは A_ACCOUNT・存在しない A program で watch 完走（A 起動 0）。A/B で A_ACCOUNT 無しは拒否、与えれば通る。
- `variant_b0_sha256_is_passed_to_preflight_and_a_wrong_hash_is_rejected`: 未指定は拒否、誤った hash は届いて不一致で拒否。**正例（一致 hash が live org まで通る）は launcher 試験では未確認**（実 org の用意が要る。`variant_preflight.rs` が CLI 入口で確認済み）。
- `grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch`: 未終了は記録なし、終了なら identity・job ID に結んで収集し採点を通る。別 trial の記録・台帳に無い job・控えの改変・成果物改変は拒否。scheduler 応答は ssh stub の固定応答（実 cluster には問い合わせていない）。

## 未解決・提案
- `INITIAL_TREES` の受け渡しは引数の追加のみ（launcher 試験で値の到達は見ていない。CLI 側の試験は既存）。
- 本番 launcher.env への `GLOBAL_LEDGER`・`VARIANT_B0_SHA256` の追記は人の運用（本番への移行）。

## 検査結果
- `bash scripts/dev/test-parallel.sh`: exit 0（6192 passed, 0 failed, 15 ignored）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
