---
title: OAuth 切れのアカウントを check と routing で検出し、選び続けない
updated: 2026-10-17
status: done
tasks: [01M544JTS13PDN3EJHTDE4YX97]
---

# account-auth-failure

設計は [ADR 2026-10-17-account-auth-failure](../adr/2026-10-17-account-auth-failure.md)。

## 変更

- 分類: `crates/task-worker/src/provider.rs`（AuthFailed の語を追加）
- 保持と復帰: `task-dispatch` `accounts.rs`（`account_cooldown_for_failure`・`auth_failed_held`）、`record_account_login`、
  celeris の `LoggedIn` event と codex login 完了
- 回数の非消費: `worker_finish.rs`・`review_verdict.rs`
- 通知: `route_failure.rs::notify_account_auth_failed`（BadNews）
- 一覧: `snapshot.rs`・`task-api` `handlers/accounts.rs`（`logged_in`）

## 証拠

最終の gate 結果は本ファイル末尾（検査の節）。

## 未解決・提案

- 暗黙アカウント（API 型 source）の認証失敗は従来どおり短い cooldown で、requeue 上限の免除だけ受ける。
  API key 切れの通知と復帰経路は別途。

## 検査

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0（nextest 5600 passed / 0 failed、doctest exit 0）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
