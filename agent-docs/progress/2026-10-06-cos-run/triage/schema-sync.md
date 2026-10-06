---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
completed: 2026-10-06
---

# Triage schema sync

`NotificationKind` に `cos_escalation` と `cos_fallback` が追加されたため、API schema と生成型を同期した。Rust の動作・`crates/` は変更していない。

## 証拠

- `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated` — 成功（1 passed）。`docs/api/v1/api-v1.schema.json` を再生成。
- `node web/scripts/gen-types.mjs` — 成功。`web/api/generated/schema.json` と `types.ts` を更新。
- `node web/scripts/gen-types.mjs --check` — 成功。
- `cargo test -p task-api --lib committed_schema_matches_generated` — 成功（1 passed）。
- `git diff --check` — 成功。
- `git diff --name-only "$CELERIS_WU_BASE"` — schema と生成型の 4 ファイルのみ。`git diff -- crates/` は空。

GUI の `NotificationKind` union に `cos_escalation` と `cos_fallback` を追加した。GUI の pnpm 依存関係再生成は行っていない。
