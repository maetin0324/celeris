# browser web query・model・route の土台

---
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
---

## 変更

- `web/features/browser/browser-model.ts`: D3.2 の Live View 不可理由 7 種、同一 origin の live path 検査、D3.3 の phase 表示・lease 秒数・15 秒での強調判定、wait 種別と一時停止 badge を定義した。
- `browser-query.ts`: gateway の `/browser/runs`、control、waits、identities、owner-session を呼ぶ query と mutation を定義した。本人 session の CSRF token を mutation に渡し、control には version と `crypto.randomUUID()` の idempotency key を付ける。変更系の同時送信を拒否し、成功後に browser query を再取得する。既存 SSE の `browser_updated`・`browser_wait_*` でも同じ query 群を invalidate する。
- `web/routes` の 3 route を ScreenFrame の骨組みとして追加し、SPA route・生成 route tree を更新した。nav の work 群の末尾に「ブラウザ」を 1 行追加した。画面の中身は後続の `runs-screen`・`run-screen`・`identities` WorkUnit が差す。

## 検証

| コマンド・確認 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile --store-dir .pnpm-store` | 成功、278 package を local store から再利用。作業用 store copy は検査後に削除 |
| `corepack pnpm@12.6.0 -C web build` | 成功。`routeTree.gen.ts` 再生成 |
| `corepack pnpm@12.6.0 -C web typecheck` | 成功 |
| `corepack pnpm@12.6.0 -C web lint` | 成功。既存の警告 5 件のみ |
| `corepack pnpm@12.6.0 -C web test` | 成功。Vitest 61 files / 369 tests、gateway 52 tests |
| `corepack pnpm@12.6.0 -C web exec vitest run features/browser/browser-model.test.ts features/browser/browser-query.test.ts` | 成功。2 files / 8 tests |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 成功。route は 13/17/22 行 |
| `WEB_E2E_WORKERS=1 corepack pnpm@12.6.0 -C web e2e e2e/parity/shell.spec.ts` | 成功。6 tests |
| `corepack pnpm@12.6.0 -C web mobile-audit --only /` | 成功。4 幅 |
| `WEB_E2E_SCOPE=nfr WEB_E2E_WORKERS=1 corepack pnpm@12.6.0 -C web exec playwright test e2e/a11y/axe.spec.ts` | 成功。既存 32 画面 |
| 一時 Playwright 台本で 3 route × 360/390/412/1440px を開き、HTTP 200・見出し・横溢れなし・nav 1 行・axe serious/critical 0 を確認 | 成功。台本は削除 |

変更前は 3 route とも 404。base commit の archive から `before/` に 12 枚、変更後を `after/` に 12 枚撮影した。場所は `/local/celeris/data/workspaces/01M470CJT10MV8XVFEGSMCXHYS/wu/query/artifacts/`。

## 後続へ

- gateway の control command では `holder` が必要。`newControlHolder()` の UUID を takeover から renew・resume まで維持し、`lease_holder` と比較して本人の操作中かを表示する。
- 後続の画面は `safeLivePath()` を通した `live_path` のみ iframe/link に使う。画面離脱時の beacon と pause 状態の再表示は run-screen 側で実装する。
- web の既存 `screens.ts` 台帳には、この葉の 3 route は未登録。画面の完成時に fixture・台帳・全画面 mobile/a11y を追加する。

## 再実行（attempt 2 の check 不合格の調査、2026-10-05）

- `pnpm -C web test` の `components/content/artifact-preview.test.tsx` の hook timeout（30s）は負荷時の一過性。loadavg 9 で typecheck・lint・test（61 files / 369 tests、gateway 52）・check:boundaries・build を再実行して exit 0。
- `pnpm -C web e2e e2e/shell/` の `home-layout.spec.ts:26`（360x800）は「ページが viewport より高くない」で overflow 3px。nav 行を外して build し直しても同じく落ちる（この葉の変更と無関係、main の 33465896 由来）。原因は `features/home/console-region.tsx` の枠の高さ下限 `MIN_VISIBLE + covered`（stale 電話幅の会話本文を見せる fix）が 360x800 の残り高さを超えること。home-stale-viewport の要求と home-layout の要求の調停が要り、この葉の範囲外なので plan_issue で申告した。
