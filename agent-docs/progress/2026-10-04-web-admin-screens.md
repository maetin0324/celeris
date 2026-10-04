---
title: 管理画面群（org・knowledge・accounts・providers・clusters・daemon・releases・help・login）の統合検証
tasks: [01M440S16A0E49172DK6ST4QPK]
status: done
completed: 2026-10-04
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

差分範囲: admin-gates 当時は `git diff --name-only ac78fad613ad HEAD` が許可範囲の外に 0 件だった。後続の `merge main`（06a389b4）で `crates/` を含む main の差分が親 branch に入ったため、この旧基点による最終チェックは 0 件にならない。admin-record の作業基点 `06a389b4` からの差分に `crates/` は無い。

### 直したもの

- web の検査は全部初回で通った（コード変更なし）。
- 文書検査 `sh scripts/dev/progress-index.sh --check` が exit 1（`2026-10-04-web-inbox-notifications.md` の front matter に `updated` が無い。主画面 2 の記録 20d00369 由来）。`updated: 2026-10-04` を 1 行足して exit 0。`check-doc-links.sh`・`check-adr-numbers.sh`（138 files）・`check-doc-layout.sh scripts/dev/docs-layout.tsv` は exit 0。

### 要望（範囲外）

- `web/styles.css` 174〜177 行の `!important` 4 件が biome の warning として残る（exit 0 なので gate は通る）。styles は範囲外のため直していない。

## admin-record: 横断の一貫性

管理画面は Celeris の運用者が設定と稼働状態を確認し、必要な変更を安全に確定する workbench として見た。入口は管理 nav、最初の判断は対象と現在状態の確認、成功は変更結果を対象の一覧や状態欄で読めること。スマホでは状態と操作を先に読み、詳しい診断は後に続く構造を基準にした。

| 観点 | 所見 |
| --- | --- |
| 表の列順 | 対象を先頭、状態を近く、最終確認・問題を後ろに置く構造で揃っている。リリースは危険操作を対象の隣に置く。幅 360 では表の一部を局所スクロールし、同じ対象の操作 card を下に出す。 |
| 状態表示 | `StatusBadge` は昇格の実行中・完了・失敗に使い、account・provider・cluster・skill の固有状態は意味色と日本語ラベルの `Badge` に写している。色だけに依存しない。 |
| 破壊的操作 | 組織・アカウント・実行枠・secret の削除を共通 `ConfirmDialog` に統一した（50d5f354）。対象、影響、戻し方、結果の確認先を出し、初期 focus は「戻る」。失敗は dialog に残す。skill の削除・外す操作、release の昇格も同じ確認を通る。 |
| form | 可視 label、項目別 error、送信中の無効化、保存ボタンを確認した。org の編集・作成は空の必須欄で保存を無効にし、API の失敗を操作の近くに出す。 |
| モバイル | 360/390/412 の screenshot と `mobile-audit` で横溢れと target を確認。login・help は主要操作と見出しが先に見える。 |

`agent-docs/web/feature-parity.md` の管理画面 R04・R06・R07・R14〜R16・R29〜R31・R33・R34・R36 の状態欄を 50d5f354 に更新した。route 行は 42 のまま。

### screenshot

- before: `/local/celeris/data/workspaces/01M440S16A0E49172DK6ST4QPK/wu/admin-record/artifacts/before-admin`
- after: `/local/celeris/data/workspaces/01M440S16A0E49172DK6ST4QPK/wu/admin-record/artifacts/after-admin`
- `corepack pnpm@12.6.0 -C web build` の後に撮影。台本は 32 画面 × 360/390/412/1440。fixture 名の重複があるため保存ファイルは各 124 枚。対象の org・knowledge・accounts・providers・clusters・daemon・releases・help・login は各 4 幅で存在する。

### 未解決・要望（範囲外）

- 既定 screenshot fixture の `/org` は 4 幅とも取得失敗と再試行を写す。org の正常な木・詳細・削除・skill 操作は `web/e2e/admin/org.spec.ts` の専用 fixture で検証した。`web/e2e/support/screens.ts` と fixture gateway はこの葉の許可範囲外。台帳の既定 fixture に組織データと effective profile を加えてほしい。
- `/daemon` の既定 fixture は dispatcher の状態と版が無い。実際の稼働・停止・更新時刻の表示を比較できる fixture を加えてほしい。`/clusters` と `/providers` の表は狭幅で局所スクロールするため、列が多いデータの視覚比較には複数行の fixture が要る。
- `web/styles.css` の既存 `!important` 4 件は lint warning のまま。変更範囲外であり、exit 0 を妨げない。
- 最終受け入れ条件の旧基点 `ac78fad613ad` による範囲 check は、先行 `merge main` が取り込んだ `crates/` などを数えるため失敗する。この葉は `crates/` に触れていない。WU 基点 `06a389b4` からの範囲 check でこの葉の責任範囲を確認する。

### 提案

- provider 冒頭の実装語を含む長い説明を短い判断文へ詰め、adapter / harness・tier の詳細は各実行枠の設定欄へ置く。小幅では説明が操作より上に長く続く。
- screenshot 台本に画面の取得完了待ちを加え、既定 fixture での成功・失敗を明示して保存する。画像枚数だけでは正常な操作状態を証明できない。

### 最終検査

admin-record の変更後に再実行した。すべて `corepack pnpm@12.6.0 -C web <script>` を使用し、`build` の後に走らせた。

| 検査 | 結果 |
| --- | --- |
| `build` | exit 0（bundle 500 kB 超の警告） |
| `typecheck` | exit 0 |
| `lint` | exit 0（既存 `styles.css` の warning 4 件） |
| `test` | exit 0（vitest 49 files / 315 tests、node:test 42 pass） |
| `check:parity`・`check:boundaries`・`check:secrets` | 各 exit 0 |
| `mobile-audit` | exit 0（31 path × 4 幅） |
| `e2e --retries=0`（full） | exit 0（118 passed / 8 skipped / 0 failed、2.3 分） |
| `check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check` | 各 exit 0 |
| 管理画面全体の生の色・任意値 grep | 0 件 |
| feature-parity route 行 | 42 行（変更なし） |

full e2e の初回は配布物試験 1 件だけ失敗した。作業中に `.pnpm-store/v11` を消したことが原因で、store を復元して full e2e を通し直した。再実行では配布物の offline install を含め全件が通った。

## 範囲 check と main 取り込み

merge `06a389b4f0103375712b13ac997ac12777caaec0` は main `864f5d29b07c558b22b9c7ac403fd8a80ccd83bf` を取り込んだ。merge の第 1 親は `8c937a020d0d5b5944f7ee8df0c5526b999276d7`。次の二つの WU 固有区間を受け入れ条件 3 の許可範囲正規表現で絞り、どちらも範囲外 0 件だった。

```sh
git diff --name-only ac78fad613ad 06a389b4^1 | grep -v -E '^(web/features/(ops|org|knowledge|help)/|web/routes/(accounts|clusters|daemon|providers|releases|help|login|org\.|knowledge\.)|web/e2e/parity/(ops|org|knowledge|help)\.spec\.ts$|web/e2e/admin/|agent-docs/progress/|agent-docs/adr/|agent-docs/web/feature-parity\.md$)'
git diff --name-only 06a389b4 HEAD | grep -v -E '^(web/features/(ops|org|knowledge|help)/|web/routes/(accounts|clusters|daemon|providers|releases|help|login|org\.|knowledge\.)|web/e2e/parity/(ops|org|knowledge|help)\.spec\.ts$|web/e2e/admin/|agent-docs/progress/|agent-docs/adr/|agent-docs/web/feature-parity\.md$)'
```

両コマンドとも出力 0 行（`grep` は一致なしの exit 1）。対して同じ範囲式で `git diff --name-only ac78fad613ad 06a389b4` を調べると範囲外は 48 file。これらは merge commit の main 側の親 `864f5d29` 由来で、Rust の `crates/`、API 文書・設定、GUI/API 生成物、ADR 検査スクリプト、web の共通 e2e 高速化・realtime/parity 基盤（`web/package.json`・Playwright 設定を含む）に分類される。範囲外の一覧は merge 差分にのみ現れ、前後の WU 固有区間には現れない。

人の回答（accept-scope）: **a — main 由来の差分を除いて条件 3 を満たしたとみなす**。このタスク自身の両区間で範囲外 0 件を確認したため、main 取り込みは revert せず、コード変更も行わない。

## main 取り込みを revert しない判断

人の回答（scope-fix）: **revert しない**。main 取り込み 06a389b4（864f5d29 の取り込み）は残し、revert commit は作らない。コード（web/・crates/）は変更しない。

- 新しい条件 3 の式（task 自身の commit だけを見る）: `git log --format= --name-only ac78fad613ad..HEAD --not main | sort -u | grep -v '^$' | grep -v -E '^(web/features/(ops|org|knowledge|help)/|web/routes/(accounts|clusters|daemon|providers|releases|help|login|org\.|knowledge\.)|web/e2e/parity/(ops|org|knowledge|help)\.spec\.ts$|web/e2e/admin/|agent-docs/progress/|agent-docs/adr/|agent-docs/web/feature-parity\.md$)'` → 出力 0 行（範囲外 0 件）。同じ区間で `crates/` に触れた file は 0 件（`git log ac78fad613ad..HEAD --not main -- crates/`）。
- 旧い式 `git diff --name-only ac78fad613ad HEAD` は 89 file、うち範囲外 48 file、`crates/` 差分 23 file。これは main 取り込み 06a389b4 で入った差分で、task の commit は含まない。旧い式は main 取り込みを残す限り満たせない。
- task の受け入れ条件は人の決定でも書き換わらない。final review が旧い式で元の条件を再評価すると落ちうる。完了の扱いは人の決定に従う。
- 検査: 文書検査 3 本（`sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`）は各 exit 0。管理画面 file 全体の生の色・任意値 grep は 0 件（exit 1 = 一致なし）。web の build 後の検査と full e2e は前回の review（同じ HEAD 2a773b01、記録上の結果は上の節と同じ）で exit 0 を確認済み。このコードは変えていない。

提案（上位 task へ）: 上位 task の受け入れ・範囲 check は、固定 sha 基点の `git diff --name-only <base> HEAD` ではなく、`git log --format= --name-only <base>..HEAD --not main` の式で書く。main を取り込んだ後も task 自身の変更だけを見られる。main 取り込みを戻す必要が出た場合は、revert の revert か main の再 merge で 864f5d29 の変更を戻す（人が決める）。
