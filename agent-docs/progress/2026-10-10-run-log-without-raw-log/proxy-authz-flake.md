# Proxy authz SSE 待ちの競合修正

tasks: [01M4HYSXM9JAHYQF78QCRS58HN]

## 原因

`proxy-authz.spec.ts` は `/browser` を開いた後、daemon の `/api/v1/stream` 要求数が増えるのを待ってから `browser_updated` を送っていた。しかし `/browser` 自身の EventSource が先にこの stream を開くため、その要求数の増加は後から開始する `page.evaluate` の `/events` fetch が購読した証拠にならない。評価対象の fetch が中継を開始する前に event が送られると、fake daemon は過去 event を再送しないため SSE の待ちが完了しなかった。

Gateway の `/events` 中継は daemon の upstream stream 応答を受けてから event-stream header を返す。そこでテストは、評価対象 fetch 自身の response header 到着後に `window.__proxyAuthzStreamReady` を立て、Playwright がその出来事を待ってから `sendEvent` するよう変更した。event 到着後の SSE 内容・raw live URL の非包含、JSON の非包含、mutation の 403 の各 assertion は維持した。gateway に購読前 event の欠陥はなく、server 変更・追加 server 試験は不要。

## 検査結果

- `TMPDIR=/tmp pnpm exec playwright test e2e/browser/proxy-authz.spec.ts` — 3 passed。
- `TMPDIR=/tmp WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/runs e2e/browser` — 28 passed。
- `pnpm exec node --test server/*.test.mjs` — 78 passed。
- `git diff --check` — pass。
- 前回 run 後の指定 check は、`proxy-authz.spec.ts` の Biome formatting 差分で lint が exit 1 だった。spec を Biome で整形して再実行し、`cd web && timeout 900 node --test server/*.test.mjs && cd .. && corepack pnpm@12.6.0 -C web run lint && corepack pnpm@12.6.0 -C web run typecheck` は exit 0（node 78 passed、lint は既存 styles.css の4 warnings、typecheck pass）。
- 整形後に `TMPDIR=/tmp corepack pnpm@12.6.0 -C web exec playwright test e2e/browser/proxy-authz.spec.ts` — 3 passed、`TMPDIR=/tmp WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/runs e2e/browser` — 28 passed、`git diff --check` — pass。
- 初回の spec 実行は run の長い TMPDIR に由来する Unix socket `listen EINVAL` でテスト起動前に失敗した。短い `TMPDIR=/tmp` で再実行し、合格した。


## 再確認（2026-10-10）

- `corepack pnpm@12.6.0 -C web exec biome check e2e/browser/proxy-authz.spec.ts` — pass。
- `TMPDIR=/tmp corepack pnpm@12.6.0 -C web exec playwright test e2e/browser/proxy-authz.spec.ts` — 3 passed。
- `TMPDIR=/tmp WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/runs e2e/browser` — 28 passed。
- `cd web && timeout 900 node --test server/*.test.mjs` — 78 passed。
- `git diff --check` — pass。
