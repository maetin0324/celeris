---
title: web/ SPA スマホ版の画面下タブの 3 列目をボードにする
tasks: [01M4CRGVA0Z9T2SQ2QR4XFR6H5]
status: done
updated: 2026-10-08
completed: 2026-10-08
---

# web/ SPA: スマホ版の画面下タブの真ん中（3 列目）をボードにする

人の依頼（CoS チャット 2026-10-08）: スマホ版の下メニューの真ん中をタスク一覧ではなくボードにする。

## 変更点

- `web/components/shell/nav-items.ts` の `mobileTabs` 3 列目を `/board`（ボード、`kanban` アイコン）にした。並びは ホーム・受信箱・ボード・案件・その他。`/tasks` は `mobileOtherItems`（その他のシート）へ `navItems` の順で落ち、`/tasks` では「その他」が現在地になる。md 以上の側面 nav は不変。
- e2e（`web/e2e/shell/tabbar.spec.ts` ほか）を新しい並びに直し、`/board` が 360px で最初の 1 画面に入ることを first-screen の方式で確認。
- ADR `2026-10-06-web-bottom-tabbar-first-screen` D2 を追記で改訂（人の決定 2026-10-08）、`docs/frontend/DESIGN.md` とコードコメントを更新。
- `/models`・`/browser/settings` の操作要素を 44×44px 以上にして mobile-audit を通した（audit-fix）。

## 各葉の証拠

- tabbar: `e2e:all e2e/shell/ e2e/chat/ e2e/work/first-screen` 83 passed（`2026-10-08-web-mobile-tab-board/tabbar.md`）。
- docs: ADR D2・DESIGN.md 改訂（`.../docs.md`）。
- audit-fix: mobile-audit exit 0（`.../audit-fix.md`）。
- web-verify: lint・typecheck・test（Vitest 608・server 77）・e2e functional 313 passed・shell/chat/first-screen 83 passed・build・mobile-audit（38 path × 4 幅）すべて exit 0（`.../web-verify.md`）。

## 検証済み sha

- sha12: **edb778e05fbb**（web-verify 後の HEAD edb778e05fbbb24a3dcc4512f55dedfd669edf27）。
- `release.sh`: exit 0。gate.json ok=true、schema_version=59。
- `verify.sh`: exit 1、**verify.json ok=false**（live_ok=false）。落ちた 2 件はどちらも web と無関係:
  - `n-1-compat`: 本番 current 0b1cefac7ff7 は schema 57 で、この sha（main 由来の schema 59）の DB を開けない（`db schema version 59 is newer than the 57 this binary supports`）。本番を 59 以上へ昇格するまで解消しない既知の状態。
  - `gui-e2e`: この run の sandbox に `@playwright/test` が無い（release の gui は `--prod` install）。環境起因。
  - 通った: start-and-migrate, counts-match, main-gets, gui, smoke, web-app-start。
- 成果物: `/local/celeris/data/workspaces/01M4CRGVA0Z9T2SQ2QR4XFR6H5/wu/release/artifacts/release/{gate.json,verify.json,sha12.txt}`。

## screenshot

`/local/celeris/data/workspaces/01M4CRGVA0Z9T2SQ2QR4XFR6H5/wu/web-verify/artifacts/screenshots/`: `board-360.png`・`board-1440.png`・`tasks-360.png`・`tasks-1440.png`。

## 人の昇格手順

promote.sh は実行していない。GUI のリリース画面で `edb778e05fbb` を確認して昇格する。昇格は本番 DB の schema 59 への migration を伴うので、バックアップ後に行う。昇格後は verify を再実行すれば n-1-compat は新 current 基準になる。

## 未解決事項

- verify.json ok=false（上記 2 件。web の変更が原因ではない）。
