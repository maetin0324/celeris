---
title: Web の設定画面・routing 画面の API path 二重前置（404）
status: done
updated: 2026-10-22
completed: 2026-10-22
tasks: [01M5JAYKJ5W80FD5954CXHDNEX]
---
# Web の設定画面・routing 画面の API path 二重前置（404）

- 原因: web の api client の規約は `/api/x`（gateway の server/relay.js が daemon の `/api/v1/x` へ前置する）。設定画面・routing 画面が `/api/v1/...` を渡していたので daemon には `/api/v1/v1/...` が届き 404 になった。
- 点検（web 全体）: 同じ誤りが他に 2 か所あった。
  - `api/realtime/transport.ts` の接続確認 `/api/v1/health`（404 でも "ok" 扱いなので表に出ていなかった）→ `/api/health`。
  - 統合操作 `features/tasks/task-integration.tsx`（task 詳細の `integrationActionPath` と、受信箱が daemon の `answer.native.path`＝`/api/v1/tasks/…/merge` をそのまま使っていた）→ `/api/tasks/…`、daemon 由来の path は `apiPathFromDaemon` で直す。
- 再発防止: `assertApiPath` が `/api/v1`（`/api/v1/`・`/api/v1?`・`/api/v1` 単独）で始まる path を TypeError で拒む。`api/client.test.ts` の source scan が client code（試験・生成物以外）に `"/api/v1` の文字列 literal が無いことを確かめる。
- 試験: 設定・routing 画面は query を `configFieldsQuery` などに切り出し、`api/fake-gateway.ts`（relay と同じ前置で fixture を引く）で要求 path と画面の表示を単体試験。e2e `e2e/admin/settings-routing.spec.ts` は本物の gateway＋fake daemon で値の表示と `/api/v1/v1/` が無いことを確かめる。

## 証拠
- `pnpm exec vitest run`（web）: 96 files・676 tests pass。
- `pnpm typecheck`・`pnpm exec biome check .`: exit 0（warning 4 件は既存）。
- `playwright test e2e/admin/settings-routing.spec.ts`: 2 passed。shell/realtime/help/gateway-relay: 15 passed。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 報告本文に記載。

## 本番への移行
- 無し（web の release で反映）。
