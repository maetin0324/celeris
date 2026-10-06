---
task: 01M44ZK8GADYD9PVAYBY73F6YC
unit: schema
status: done
completed: 2026-10-05
---
# Phase 2: API schema と gui/web の生成型の再生成（p2-schema）

## 変更

- `docs/api/v1/api-v1.schema.json`: 再生成不要だった（config-api 段で `UPDATE_SCHEMA=1 cargo test -p task-api` が実行済み。`committed_schema_matches_generated` が一致を裏付け）。
- `gui/app/celeris/types.ts`: `pnpm gen:types`（corepack pnpm@11.27.0、worktree なので先に `pnpm install --offline`）で再生成。Phase 2 の新欄（`ExcludedReason`、`RoutingTraceV1` の `account_id`/`model`/`source_id`、`CandidateTrace` の費用成分と除外理由、`LlmSourceStateView` 等の追加）が入る。
- `web/api/generated/types.ts` と `web/api/generated/schema.json`: `node scripts/gen-types.mjs`（corepack pnpm@12.6.0、先に `pnpm install --offline`）で再生成。
- event 型（`event.schema.json`）・event-kinds・invalidation-map は触っていない（`RoutingDecided` の event 型は Phase 2 で変更していない）。

## 証拠

- `cargo test -p task-api --lib schema`: 3 passed（`committed_schema_matches_generated` 含む）。
- gui/web いずれも 2 回連続で `gen:types` を実行し、2 回目で差分なし（生成物が定常）。
- `pnpm -C web typecheck`（corepack pnpm@12.6.0）: exit 0。
- 差分は生成物の 3 ファイルのみ（`git diff --stat` で確認）。コードは書かない。

## 未解決事項

- `pnpm -C gui typecheck` は失敗するが、これは gui unit 自身のファイル（`TaskRoutingPanel.tsx`・`task-routing.ts` が旧 `RoutingAudit` を参照）のため、本 unit の範囲外。gui unit の作業で直る（`RunRoutingAudit` への移行）。
