# web-verify

tasks: [01M4CSWN97P4TBRNZXTME04R98]

完了日: 2026-10-08。audit-fix 統合後（4eed5b95 を ff で取り込んだ HEAD）で検証した。コードは変えていない。

## 実行結果

- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --prefer-offline` — exit 0。
- `... lint` — exit 0（既存の `styles.css` `!important` 4 件の warning のみ）。
- `... typecheck` — exit 0。
- `... test` — exit 0（Vitest 608 件、server test 77 件、失敗 0）。
- `... e2e`（functional）— exit 0（313 passed、8 skipped）。
- `... e2e:all e2e/shell/ e2e/chat/ e2e/work/first-screen`（nfr を含む）— exit 0（83 passed）。
- `... build` — exit 0。
- `... mobile-audit` — exit 0（38 path × 4 幅 ok）。

## スクリーンショット

`pnpm -C web screenshots --only /board|/tasks --viewport --widths 360,1440 --out <dir>`。`board-360.png`・`board-1440.png`・`tasks-360.png`・`tasks-1440.png` を WU 成果物 `.../wu/web-verify/artifacts/screenshots/` に置いた。360px の /board で下部タブの「ボード」が現在地表示（目視確認）。

## 未解決事項

- 初回の試行では base に audit-fix が入っておらず mobile-audit が /models・/browser/settings で落ちた。4eed5b95 を取り込んで解消した。
