---
title: ホーム（/）の初回表示の空白帯の修正（fix-home-r2）
tasks: [01M456GWPPBTZ339CASNC3EA2T]
status: done
updated: 2026-10-05
---

# ホーム（/）の初回表示の空白帯の修正（fix-home-r2）

## 原因

fixture gateway（rich profile）と実ブラウザ（Chromium）で / を初回表示して測った（修正前）。

| viewport | window.scrollY | h1 の top | 判断待ち先頭の top | document の高さ |
| --- | --- | --- | --- | --- |
| 360x800 | 253 | -184 | -73 | 1053 |
| 1440x900 | 21 | 3 | 116 | 921 |

空白帯の正体はページの自動 scroll。`features/console/console-view.tsx` の `useFollowPage` は「末尾にいる」を初期値にして、会話の block が届くたびに **window** を document の末尾へ送る。ホームでは会話の下に `pb-40`（fixed の送信欄の逃げ）があるため、document は viewport より高くなり、初回表示でページが末尾へ送られる。結果、上にあった header（360 では上帯、1440 では sidebar 側の上端）・h1・判断待ちが上へ押し出され、下には会話の後の `pb-40` の空白が見える。1440 でも 21px 押し上がっていた。

## 修正

- `web/features/home/console-region.tsx`（新規）`ConsoleRegion`: 会話（ConsoleView）を独立した scroll 領域にする。枠の高さを「viewport − 枠の上端 − main の下の余白・Console slot」に合わせて実行時に設定し（runtime の viewport 補正、ResizeObserver と resize で追従）、document を viewport に収める。document が溢れないので ConsoleView の `window.scrollTo` は何もしない。
- 枠の中の追従: 末尾にいる間だけ追記に合わせて枠の末尾へ送る。末尾は `pb-40` のうち送信欄に隠れる分までとし、最後の block を送信欄のすぐ上に置く（空白を見せない）。枠の上へ離れている間に追記が来たら、送信欄の上に「最新へ」（chevron-down、Button size sm）を出す。
- `web/routes/index.tsx`: ConsoleView を ConsoleRegion で包むだけ。h1『ホーム』・nav『受信箱と通知』・リスト『期限の近い判断待ち』・URL は変えていない。
- 生の色・任意値の class は足していない（契約の grep で 0 件）。高さと「最新へ」の bottom は実行時に測る値なので style に直接書く。
- features/console・components/shell・styles.css は変更していない。

修正後:

| viewport | window.scrollY | h1 の top | 判断待ち先頭 | 入力欄 | document の高さ |
| --- | --- | --- | --- | --- | --- |
| 360x800 | 0 | 69 | 180–251 | 726–792 | 800 |
| 1440x900 | 0 | 24 | 137–183 | 826–892 | 900 |

## before / after の screenshot

- before: `/local/celeris/data/workspaces/01M456GWPPBTZ339CASNC3EA2T/wu/home-layout/artifacts/home-before/home-360.png`・`home-1440.png`
- after: `/local/celeris/data/workspaces/01M456GWPPBTZ339CASNC3EA2T/wu/home-layout/artifacts/home-after/home-360.png`・`home-1440.png`
- 撮影 script: `/local/celeris/data/workspaces/01M456GWPPBTZ339CASNC3EA2T/wu/home-layout/artifacts/home-shot.mjs`（fixture gateway を使う。web/scripts から実行した）

## check 結果

- `corepack pnpm@12.6.0 -C web e2e shell/home-layout.spec.ts` → 3 passed（360x800・1440x900 の初回表示、会話の枠の追従と『最新へ』）。修正前の index.tsx に戻すと 2 本が `window.scrollY` 253 / 21 で落ちることを確認した。
- `e2e shell/ parity/shell.spec.ts parity/console.spec.ts parity/inbox.spec.ts work/inbox-notifications.spec.ts` → 34 passed / 2 skipped / **1 failed**（下の未解決事項）。
- `e2e a11y/axe.spec.ts parity/mobile-gate.spec.ts` → 64 passed。
- `mobile-audit --only /` → 4 幅 ok。
- `typecheck`・`lint`（警告は既存の 5 件のみ）・`test`（42 pass）・`check:boundaries` → exit 0。

## 未解決事項

- `web/e2e/parity/console.spec.ts` の「parity: / Console 追記の追従と『最新へ』」が落ちる。この試験は / のページ（window）が scroll する前提で、`window.scrollTo(0, 0)` の後に追記して ConsoleView の『最新へ』を待つ。ホームの会話を独立 scroll 領域にした今回の方針（document の scroll 0）とは両立しない。parity/console.spec.ts は今回の変更範囲外なので直していない。同じ観点（追従・『最新へ』・ページ不動）は `web/e2e/shell/home-layout.spec.ts` の 3 本目が枠に対して確かめている。

## 提案

- parity/console.spec.ts の上の試験を、/ では `[data-home-console]` の枠を scroll させ、枠の末尾との距離と『最新へ』を見る形に直す（または /org/:id の Console で window 前提の試験を残す）。
- ConsoleView に scroll 容器を渡せる口（追従の対象を window か要素かで選ぶ）を作り、home の追従と『最新へ』を ConsoleView 側に一本化する（features/console の変更になるので別の葉）。
