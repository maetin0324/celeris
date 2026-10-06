---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: gui
status: done
completed: 2026-10-05
---

# Phase 4 GUI: shadow routing の監査表示

`GET /tasks/{id}/routing` の schema から GUI 型を再生成した。task routing panel は run に結び付いた shadow を primary の監査と別欄に表示する。判断のみと実行を区別し、完了・失敗・見送り、理由、候補 source/model と primary との差、UTC 日次予約の tokens/effective USD の予約・確定消費を示す。欠測は「不明」とし、shadow 記録のない旧 run には欄を出さない。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@11.27.0 -C gui install --offline --store-dir /tmp/p4-gui-pnpm-store` | exit 0（既存の pnpm cache を書き込み可能な一時 store に複製） |
| `corepack pnpm@11.27.0 -C gui gen:types` | exit 0 |
| `corepack pnpm@11.27.0 -C gui typecheck` | exit 0 |
| `corepack pnpm@11.27.0 -C gui test` | exit 0、95 files / 1336 tests passed。`routing-shadow.json` fixture と `routing-shadow.test.tsx` を含む |
| `corepack pnpm@11.27.0 -C gui exec biome check app/components/TaskRoutingPanel.tsx test/unit/routing-shadow.test.tsx` | exit 0 |

## 備考

`corepack pnpm@11.27.0 -C gui lint` は変更外の既存ファイルの format 指摘を含み失敗したため、変更した手書きファイルに限定して Biome を確認した。
