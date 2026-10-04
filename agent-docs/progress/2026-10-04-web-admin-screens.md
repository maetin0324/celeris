---
title: 管理画面群（org・knowledge・accounts・providers・clusters・daemon・releases・help・login）の統合検証
tasks: [01M440S16A0E49172DK6ST4QPK]
status: running
updated: 2026-10-04
---

# 管理画面群の統合検証

build 段 4 葉（ops-config・ops-runtime・knowledge-help・org）を統合した branch（基点 8f1938793c55）での横断検証の記録。
葉ごとの記録は [2026-10-04-web-admin-screens/](2026-10-04-web-admin-screens/)（ops-config・ops-runtime・knowledge-help・org）。

## gates

admin-gates 葉（2026-10-04）。すべて `corepack pnpm@12.6.0 -C web <script>` を build の後に順に実行した。

| 検査 | 結果 |
| --- | --- |
| `build` | exit 0（chunk 500 kB 超の警告のみ） |
| `typecheck`（tsc -b） | exit 0 |
| `lint`（biome） | exit 0、285 files、warning 4 件（既存の `web/styles.css` 174〜177 行の `!important`。今回の差分外） |
| `test` | exit 0、vitest 49 files / 315 tests passed、node:test 42 pass / 0 fail |
| `check:parity` | exit 0 |
| `check:boundaries` | exit 0 |
| `check:secrets` | exit 0（token absent from build output, HTML, /api responses, errors and logs） |
| `mobile-audit` | exit 0（31 path × 4 幅 ok） |
| `e2e --retries=0`（full） | exit 0、247 passed / 8 skipped（fixture screenshot 5 件と real-staging 3 件。既定で skip）/ 0 failed、18.0 分 |

生の色・任意値 grep（FRONTEND_CONTRACT の §66 の式）を管理画面の file 全体
（`web/features/{ops,org,knowledge,help}`、`web/routes/{accounts,clusters,daemon,providers,releases,help,login}.tsx`、
`web/routes/org.*.tsx`、`web/routes/knowledge.*.tsx`）に当てた: 一致 0 件（grep exit 1）。

差分範囲: `git diff --name-only ac78fad613ad HEAD` は許可範囲（管理画面の features・routes・parity spec・`web/e2e/admin/`・`agent-docs/`）の外に 0 件。`crates/` は無差分。

### 直したもの

- web の検査は全部初回で通った（コード変更なし）。
- 文書検査 `sh scripts/dev/progress-index.sh --check` が exit 1（`2026-10-04-web-inbox-notifications.md` の front matter に `updated` が無い。主画面 2 の記録 20d00369 由来）。`updated: 2026-10-04` を 1 行足して exit 0。`check-doc-links.sh`・`check-adr-numbers.sh`（138 files）・`check-doc-layout.sh scripts/dev/docs-layout.tsv` は exit 0。

### 要望（範囲外）

- `web/styles.css` 174〜177 行の `!important` 4 件が biome の warning として残る（exit 0 なので gate は通る）。styles は範囲外のため直していない。

## 未解決

- screenshot after-admin・横断の一貫性検査・feature-parity の更新は後続の admin-record 葉が行う。
