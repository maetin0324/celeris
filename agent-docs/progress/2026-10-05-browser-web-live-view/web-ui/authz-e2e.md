# browser 認可・待ち・狭幅の e2e 検証

`proxy-authz.spec.ts`、`waits.spec.ts`、`narrow-a11y.spec.ts` を追加した。偽 browser backend と owner socket を使い、別 cookie session、owner grant のない session、別 task に属する run、存在しない run の Live View と control を固定コードで拒否することを確認した。generic `/api` relay の `browser_route_required`、JSON と SSE と DOM からの raw `live_view_url` 除去も確認した。

待ちは `approve_once` と `deny`、credential 登録が fixture に届いた記録と更新後の状態を確認する。credential 応答と画面には password が残らない。回答後の再取得で form が閉じても結果が読み取れるよう、待ちカードの状態を日本語で表示する。360×800 では `/browser` と run 画面の縦順、sticky control bar、44px の操作領域、横溢れ、axe の serious / critical 違反がないことを確認する。時間経過待ちは使わず、応答、fixture の記録、SSE 接続の出来事で同期した。

## 検証

- `WEB_E2E_SCOPE=functional WEB_E2E_WORKERS=2 pnpm -C web exec playwright test e2e/browser/proxy-authz.spec.ts e2e/browser/waits.spec.ts e2e/browser/narrow-a11y.spec.ts` — 8 件成功。
- `WEB_E2E_SCOPE=functional WEB_E2E_WORKERS=2 pnpm -C web exec playwright test e2e/browser` — 17 件成功（browser 全体を一度に実行）。
- `pnpm -C web typecheck` — 成功。
- `pnpm -C web build` — 成功。
- `pnpm -C web exec biome check e2e/browser/proxy-authz.spec.ts e2e/browser/waits.spec.ts e2e/browser/narrow-a11y.spec.ts features/browser/browser-waits-panel.tsx` — 成功。
- `pnpm -C web exec vitest run features/browser/browser-waits-panel.test.tsx` — 12 件成功。
- 360・390・412・1440px の `/browser` と run 画面を、変更前後とも run の成果物ディレクトリの `screenshots/` に保存した。360px と 1440px を目視し、状態・操作の順と横溢れに問題はなかった。

追加 spec の初回実行では試験側の locator、SSE の接続順、credential 成功後に form が消えることの想定で 3 件失敗した。全 browser 試験では decision の成功表示が再取得で消える不具合を見つけ、待ちカードに完了状態を残して修正した。最終実行は 17 件成功。未解決事項はない。
