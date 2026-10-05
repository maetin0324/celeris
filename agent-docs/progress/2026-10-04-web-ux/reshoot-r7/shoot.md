---
title: reshoot-r7 shoot — after-r7 の撮影（build + screenshots 2 回）
status: done
tasks: [01M464X81ESMPAEXMAHGHHT6QX]
updated: 2026-10-05
---

# reshoot-r7 shoot — after-r7 の撮影

fix-r7 統合後の HEAD で `web/` を build してから `after-r7` を基本画面＋状態変種の全枚撮った。目視判定はこの WorkUnit では行わない（後段の look-360/390/412/1440 が行う）。

## HEAD

- `d27fc55baadb71f15af8d1cbcd2183575067b478`（`integrate wu/fix-r7 (phase fix)`）

## install（node_modules が無かったため先に実行）

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` exit 0（`/local/.pnpm-store/v11` への写しは不要だった。既存 lockfile のまま解決、660ms）

## build

- `corepack pnpm@12.6.0 -C web build` exit 0
- 出力末尾:
  ```
  dist/index.html                   1.08 kB │ gzip:   0.73 kB
  dist/assets/index-Dx8Q5Vcf.css   38.19 kB │ gzip:   7.89 kB
  dist/assets/index-CCiVWKXR.js   752.21 kB │ gzip: 221.52 kB
  ✓ built in 299ms
  ```
  （500kB 超 chunk の既定警告のみ、エラーなし）

## 撮影 1 回目: 基本画面

- `node web/scripts/screenshots.mjs --out <このWUのartifacts>/after-r7` exit 0
- 出力: `screenshots: 128 image(s) from 32 screen(s) -> .../after-r7`

## 撮影 2 回目: 状態変種

- `node web/scripts/screenshots.mjs --states --out <このWUのartifacts>/after-r7` exit 0
- 出力: `screenshots: 120 image(s) across 9 states -> .../after-r7`

## 枚数

- 合計: 244 枚（128 + 120）
- 幅別: 360 = 61、390 = 61、412 = 61、1440 = 61

## stale-_-*.png（ホーム stale、fix-r7 home-stale 節の対象）

- `stale-_-360.png`
- `stale-_-390.png`
- `stale-_-412.png`
- `stale-_-1440.png`

## long-id-*.png（長い ID、fix-r7 long-id-exec 節の対象を含む）

- `long-id-_inbox-360.png`
- `long-id-_inbox-390.png`
- `long-id-_inbox-412.png`
- `long-id-_inbox-1440.png`
- `long-id-_tasks-360.png`
- `long-id-_tasks-390.png`
- `long-id-_tasks-412.png`
- `long-id-_tasks-1440.png`
- `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-360.png`
- `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-390.png`
- `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-412.png`
- `long-id-_tasks_01M44C5GXAC1A8D95VMMRYYG0F01M44DCZWBN71Z80577YM0HTPR-1440.png`

## 参考（撮影のみ、根拠は fix-r7.md に委ねる）

- reshoot-r6.md の保留: `stale-_-360/390/412` は要修正、`long-id-..._01M44C5GX...-1440` は保留（取得失敗帯が残る）。fix-r7 はこの 2 点を狙った修正（ホーム stale の初期 viewport と長い ID 1440 の execution/routing fixture）。本 WU は撮影のみで、直否の判定は look-360/390/412/1440 と reshoot-r7.md に委ねる。
- fullPage の screenshot ではホーム stale の固定送信欄との関係は写らない（`web-spa-screenshots-mjs-fixture-mapping` 等の既知の制約）。viewport での見え方は fix-r7.md の `home-stale-viewport.spec.ts` の e2e 結果を引用する。
