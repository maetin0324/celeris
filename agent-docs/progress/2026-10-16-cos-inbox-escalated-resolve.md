---
task: 01M51B9AFNAQ8FVXM8WAJ5B3H4
status: done
completed: 2026-10-16
---
# CoS 受信箱: 元の待ちが閉じた escalated 項目を resolved にする

ADR: `agent-docs/adr/2026-10-16-cos-inbox-escalated-resolve-on-source-close.md`

## 変更
- `crates/task-core/src/chat/triage.rs`: `resolve_stale` が escalated も対象。`cos_triage_resolve_stale` 追加。
- `crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`: credential_wait にも解消だけ適用。
- 試験 2 件（triage_tests.rs）。

## 本番への移行
migration は不要。新しい release の daemon 起動時の `request_cos_triage_reconcile`（startup reconcile）が
現在の人の inbox snapshot と比べ、元の待ちが閉じた既存の escalated 257 件を resolved にする。
確認: 昇格後 `GET /api/v1/cos/inbox?state=escalated` が人の inbox 件数と一致して減ること。

## 証拠
最終節（検査結果）を参照。

## 検査結果
- `cargo fmt --all -- --check` exit 0
- `cargo clippy --workspace -- -D warnings` exit 0
- `bash scripts/dev/test-parallel.sh` exit 0（5373 passed / 0 failed）
