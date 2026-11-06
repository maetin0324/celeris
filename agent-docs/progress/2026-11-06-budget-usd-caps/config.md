---
title: WorkUnit config — celeris config を USD 欄にし旧欄を読み込みエラーにする
tasks: [01M6QDK2PMJTXF17Z8TNDKF85M]
status: done
updated: 2026-11-06
---
# WorkUnit config

## 変更
- `[project_controller.budget] max_tokens_per_day` を消し `max_usd_per_day`（有限で > 0、既定 100.0）に置き換えた。`[execution.tree] max_tree_tokens` を消し `max_tree_usd`（任意、有限で > 0）に置き換えた。旧欄は `deny_unknown_fields` で読み込みエラー。
- top-level `[budget_rates."<接頭辞>"]`（4 欄必須）を `Config.budget_rates` に追加。`Config::validate` が `task_core::pricing::BudgetRates::new` で検査し、単価表で 4 種とも単価がある接頭辞は読み込みエラー。daemon は `set_budget_rates`（dispatcher）と `ApiSettings.budget_rates` に渡す。
- 試験: 旧欄の読み込みエラー（メッセージに旧欄名。該当行に LEGACY_REJECT）、`max_usd_per_day`・`max_tree_usd` の既定と不正値、`budget_rates` の 4 欄必須・表の上書き拒否。
- `docs/ops/project-controller.md` の旧欄の言及を新欄に直した（旧欄名は書かない）。`web/e2e/parity/project-controller.spec.ts` は api-worker の変更で済んでいた。
- 範囲外の最小変更: `task-ops/src/project_budget.rs` の inline 試験が source-size gate（300 行）を超えていたため、試験を `project_budget_tests.rs` に移した（内容は変えていない）。`celerisctl` の `worker_tests.rs` の `Config` 組み立てに `budget_rates` を足した。
- `crates/campaign` に差分なし。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（6252 passed、22 skipped）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → 差分なし。
- `--all-targets` の clippy は task-core の既存試験（project_retention_scale.rs ほか、この WU が触っていない file）で落ちる。

## 未解決
- `crates/task-core/src/tree.rs` のコメントと `crates/task-api/tests/project_controller.rs`、`docs/api/v1/api.md`、`scripts/dev/long-horizon-pilot/driver.py` に旧欄名の言及が残る（他 WU の担当。closeout の走査で確認）。
