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

## 検証済み sha

- S = `3828033aca7b6a7b851df9ba56fa0854ad18f3d8`（sha12 `3828033aca7b`）。S 以降の差分は `agent-docs/progress/` だけ（この整理 commit のみ）。
- `release.sh S`: exit 0（gate.json ok、schema_version=59）。`SD_REPO=$PWD verify.sh 3828033aca7b`: exit 0、**verify.json ok=true**（live_ok=false）。
  - 通った検査: start-and-migrate、counts-match、main-gets、gui、gui-e2e（`pnpm e2e:staging`、gui に `@playwright/test` を入れて実行）、smoke、web-app-start。
  - `live_ok=false` は n-1-compat だけ。本番 current 0b1cefac7ff7 は schema 57 で、schema 59 の DB を開けない（`db schema version 59 is newer than the 57 this binary supports`）。本番 schema 57 との N-1 互換が構造的に無いためで、web の変更とは無関係。`ok` の判定（検査 1〜4・4b・6）には効かない。
- 成果物: `wu/release2/artifacts/release/{gate.json,verify.json,sha12.txt}`。

## web-recheck の結果

main 統合後の HEAD（6641bfba）での再検証（`wu/web-recheck/artifacts/web-recheck.md`）。lint exit 0、typecheck exit 0、Vitest 608 passed・server 77 passed、e2e functional 313 passed、**e2e:nfr 111 passed**（`[nfr]` project、a11y/axe・parity/mobile-gate・realtime/refetch-scope を含む）、shell/chat/first-screen 83 passed、build・mobile-audit（38 path × 4 幅）exit 0。Rust は `test-parallel.sh`（4746 passed）と clippy -D warnings が exit 0。

## screenshot

`wu/web-recheck/artifacts/screenshots/`（`/board`・`/tasks` の 360px・1440px 各 1 枚、計 4 枚）。

## 変更点

- `web/components/shell/nav-items.ts` の `mobileTabs` 3 列目を `/board`（ボード、`kanban`）にした。並びは ホーム・受信箱・ボード・案件・その他。`/tasks` は「その他」のシートに落ち、`/tasks` では「その他」が現在地になる。md 以上の側面 nav は不変。
- e2e を新しい並びに直し、`/board` が 360px の最初の 1 画面に入ることを first-screen の方式で確認。
- ADR `2026-10-06-web-bottom-tabbar-first-screen` D2 を追記で改訂（人の決定 2026-10-08）。`docs/frontend/DESIGN.md` とコメントを更新。
- `/models`・`/browser/settings` の操作要素を 44×44px 以上にして mobile-audit を通した。

## 人の昇格手順

promote.sh は実行していない。人が GUI のリリース画面で `3828033aca7b` を選んで昇格する。昇格は schema 59 への migration を伴うので、バックアップ後に行う。

## 未解決事項

- なし（live_ok=false は上記のとおり本番が schema 57 のためで、昇格で解消する）。

## 葉別の記録

葉別 4 file（docs・tabbar・audit-fix・web-verify）は repo から外し、`wu/release2/artifacts/leaf-records/` に写した。
