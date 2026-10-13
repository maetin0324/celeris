---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: run-fix: 統合後の route test helper 衝突修正
updated: 2026-10-11
---
# run-fix: 統合後の route test helper 衝突修正

## 変更

- dispatcher 試験 fixture の route 登録を `install_test_routes` に一本化し、reviewer 用の別 Registry と adapter map を重ねる helper と呼び出しを削除。
- reviewer run の route 試験は通常 worker の `llm_route_selected` 1 件を許容し、incompatible reviewer が開始せず verdict も出さないことを検証。
- 新しい llm_route 経路で意味を失った reviewer tier 固定試験 3 件を削除。
- clippy が検出した旧 provider 選択経路を production build から除外した。対象は `dispatcher.rs` の `local_provider_health`、`dispatcher/coding_default.rs` の `catalog_family`、`dispatcher/provider_select.rs` の `ASSIGNMENT_EXCLUDED`・`ProviderPick`・`LegacyProfile` と assignment/profile/rank/provider selection/sticky/account pick 一式。テスト専用の旧 fixture から参照される関数は `#[cfg(test)]` に限定した。後続 dispatch-prune はこの旧経路を再削除しないこと。

## 検証

- `cargo test -p task-dispatch` — exit 0: 902 unit tests、11 integration tests、4 unified-kill tests pass（route_select_dispatch_run/review を含む）。
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0。
- `cargo test --workspace --no-run` — exit 0。
- `cargo test -p task-core migration` — exit 0: 28 tests pass。
- `cargo test -p task-core -p task-api -p task-worker schema_matches` — exit 0: task-api committed schema match pass。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0。
- `git diff --check` — exit 0。

## 本番への移行

本番設定や daemon の変更は不要。
