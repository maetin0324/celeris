---
title: InboxKind::ALL に Unintegrated を加える
tasks: [01M51DPWFC4WQJSJ6Y0KV89VEM]
status: done
completed: 2026-10-16
updated: 2026-10-16
---
# InboxKind::ALL に Unintegrated を加える

ADR 付記: `agent-docs/adr/2026-10-16-cos-inbox-escalated-resolve-on-source-close.md`（付記 2026-10-16: `InboxKind::ALL` の取りこぼし）

## 変更
- `crates/task-ops/src/human_inbox.rs`: ALL を 17 要素にして Unintegrated を追加。
- `human_inbox/tests.rs`: 全 variant の網羅 match（`variant_index`）と ALL の突き合わせ試験、fixture に Unintegrated を追加。
- `task-dispatch tests/cos_chat_triage.rs`: 現在 0 件でも閉じた escalated unintegrated が reconcile で resolved になる試験（ALL から修正を外すと落ちることを確認）。
- `task-api inbox_notifications` の kind 検証は ALL 参照のため、`kind=unintegrated` が有効になる（コード変更なし）。

## 証拠
- `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace -- -D warnings` exit 0
- `bash scripts/dev/test-parallel.sh` exit 0（5377 passed, 0 failed, 13 ignored）

## 本番への移行
migration 不要。昇格後の startup reconcile が既存の unintegrated escalated 46 件を resolved にする。

## 未解決事項・提案
なし。
