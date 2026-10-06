---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---

# web-chat home

`/` を CoS chat の入口にし、thread の選択を `?thread=` に保存する。未指定時は直近の human thread を選び、無ければ新規作成する。会話一覧、履歴・streaming、添付、カード、composer を接続した。本文と composer を viewport 内に収め、狭い幅では下部タブバーの上で使えるようにした。旧 Console と HomeEntries は `/console` に移し、ナビと gateway の SPA route を追加した。`/console/stream` の中継は維持する。

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: ローカル store に tarball が無く失敗。続けて `corepack pnpm@12.6.0 -C web install --frozen-lockfile`: exit 0。
- `corepack pnpm@12.6.0 -C web build`: exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web lint`: exit 0（既存 styles.css の warning 4 件）。
- `corepack pnpm@12.6.0 -C web test`: exit 0（vitest 483 件、server 57 件）。
- `corepack pnpm@12.6.0 -C web check:parity`、`check:boundaries`: exit 0。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/parity/console.spec.ts e2e/shell/home-layout.spec.ts --workers=2`: 12 件通過。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test --workers=2 --reporter=line`: exit 0（245 passed、8 skipped）。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/shell/chat-home.spec.ts --workers=2`: exit 0（4 件。共有 URL、再読込、新規作成、320px と下部タブバー）。
- `corepack pnpm@12.6.0 -C web mobile-audit --only /`、`--only /console`: それぞれ 4 幅で exit 0。画面外の file input と重複する隠し link を除いてから確認した。

## 未解決・提案

- この WorkUnit の未解決事項は無い。後続 e2e-chat・e2e-mobile でチャット全操作と mobile audit を確認する。
- chat client が gateway の `/api` でなく daemon の `/api/v1` を直接呼んでいたため、browser 向け URL に修正した。添付リンクも gateway 経由にした。
