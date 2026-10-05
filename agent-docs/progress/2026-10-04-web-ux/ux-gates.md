---
title: web 全検査・full e2e（visual QA 後の task branch）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# web 全検査・full e2e（visual QA 後の task branch）

対象: branch `celeris-wu/01M44XGC0TDNPHQCPQYT9VZA6Q/gates`（基点 575202b2、visual QA 統合後）。
実行場所: `web/`（worktree）。CARGO_TARGET_DIR は使っていない（web のみ）。

## 結論

- web の既存検査は全て exit 0。
- e2e:all（`WEB_E2E_LATENCY_WORKERS=1`）は 1 回目に `states.spec.ts` の既知所見 1 件（`long-text /inbox` の 360px 横 scroll）を `test.fail` で「期待どおり失敗」として数えていた。これを web/ 内で直し（commit `3b368d81`）、所見を外して再実行し、全件 pass。
- crates/・web/server/・web/api/generated/ に task 自身の差分なし。

## 検査表

コマンドは全て `repos/agent-platform` で実行。`corepack pnpm@12.6.0 -C web <script>`。ログは `artifacts/logs/`。

| 検査 | コマンド | exit | 件数・要点 | 修正 commit |
|---|---|---|---|---|
| install | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | lockfile どおり（`Done`） | — |
| typecheck | `… -C web typecheck`（`tsc -b`） | 0 | エラー 0 | — |
| lint | `… -C web lint`（`biome check .`） | 0 | 313 files checked。warning 5 件（既存: `e2e/states/states.spec.ts` 未使用引数 1、`styles.css` の `!important` 4）。error 0 | — |
| test（vitest） | `… -C web test` の前段（`vitest run`） | 0 | 58 files / 350 tests passed | — |
| test（node --test） | `… -C web test` の後段（`node --test server/*.test.mjs`） | 0 | tests 42 / pass 42 / fail 0 / skipped 0 | — |
| check:boundaries | `… -C web check:boundaries` | 0 | 出力なし（違反なし） | — |
| check:parity | `… -C web check:parity` | 0 | 出力なし（違反なし） | — |
| build | `… -C web build`（vite build） | 0 | built。chunk size の警告のみ | — |
| check:secrets | `… -C web check:secrets` | 0 | token は build 出力・HTML・/api・error・log のいずれにも無い | — |
| mobile-audit | `… -C web mobile-audit` | 0 | 31 path × 4 幅（360/390/412/デスクトップ）ok。修正後も再実行して ok | — |
| e2e:all | `WEB_E2E_LATENCY_WORKERS=1 … -C web e2e:all` | 0 | 最終: 307 passed / 8 skipped / 0 failed（3.4 分） | `3b368d81` |

### e2e:all の project 別件数（最終実行、list reporter の行を集計）

| project | passed | failed | skipped | 備考 |
|---|---|---|---|---|
| functional | 177 | 0 | 8 | skipped は下表 |
| nfr | 95 | 0 | 0 | axe・mobile-gate・refetch-scope |
| nfr-latency | 34 | 0 | 0 | `WEB_E2E_LATENCY_WORKERS=1` で 1 本ずつ |
| release | 1 | 0 | 0 | parity/cutover（offline install で配布物が動く） |
| 合計 | 307 | 0 | 8 | Playwright の最終行と一致 |

skipped 8 件の理由（テストの `test.skip` 条件どおり）:
- `WEB_SHOTS_OUT is required`（fixture screenshot 試験 5 件: `parity/inbox.spec.ts` 2、`parity/org.spec.ts`、`parity/knowledge.spec.ts`、`parity/reports.spec.ts`）。shots WU の screenshot 撮影で使う。
- `requires WEB_E2E_REAL_BASE_URL`（`parity/real-staging-readonly.spec.ts` 3 件）。実 staging への接続が要るので、この run では外部に出ない。

### 1 回目の e2e:all（修正前）

- 307 passed / 8 skipped。ただし functional に `✘ long-text /inbox` が 1 件あった（`test.fail` の期待どおりの失敗として passed に数えられる）。
- 原因: 受信箱の「先に答える項目」の link（`inbox-screen.tsx` の `linkClass`）が `inline-flex` で `break-words` のため、区切りの無い長い語（`averyveryverylongword…`）が折り返さず、360px で `scrollWidth` が 57px 超える。診断では link（`A.inline-flex … break-words`）と親 span が右端 417px まで出ていた。`break-words` は flex の min-content を縮めないので折り返せない。
- 直し: `linkClass` の `break-words` を `wrap-anywhere`（`overflow-wrap: anywhere`、Tailwind v4 の標準 utility。min-content も縮む）に替え、`states.spec.ts` の `findings` を空にした（所見は解消）。任意値 class は使っていない。色は変えていない。`--color-input` と parity の h1・accessible name・URL は触っていない。
- 修正後の検証: `e2e/states` と `e2e/inbox` 系 31 件 pass（exit 0）。build（dist 再生成）後に e2e:all 全体を再実行して 307 passed。

## 変更の範囲

- commit `3b368d81`（`web/features/inbox/inbox-screen.tsx` 1 行、`web/e2e/states/states.spec.ts` の所見表の空化）。
- 他は変更なし。`web/e2e/parity/`・`web/api/generated/`・`web/server/`・`crates/`・DB schema は触っていない。

## crates/・web/server/・web/api/generated/ の無差分

- task 差分の確認は acceptance の式で行う（`git log --format= --name-only 7eab1be6a4e3..HEAD --not main` 等）。結果は本記録の commit 後に確認する。

## after screenshot

build（`corepack pnpm@12.6.0 -C web build`、dist 再生成）後に `web/scripts/screenshots.mjs`（偽 daemon `fixture-gateway.mjs` 経由、360/390/412/1440px の 4 幅・fullPage）で撮影。UI は変更していない。

- after（全台帳 31 画面 × 4 幅 = 124 枚）: `/local/celeris/data/workspaces/01M44XGC0TDNPHQCPQYT9VZA6Q/wu/shots/artifacts/after`
  - コマンド: `corepack pnpm@12.6.0 -C web screenshots -- --out /local/celeris/data/workspaces/01M44XGC0TDNPHQCPQYT9VZA6Q/wu/shots/artifacts/after`
  - 出力: `screenshots: 124 image(s) from 31 screen(s)`
- after-states（8 状態 × 各画面 × 4 幅 = 116 枚）: `/local/celeris/data/workspaces/01M44XGC0TDNPHQCPQYT9VZA6Q/wu/shots/artifacts/after-states`
  - コマンド: `corepack pnpm@12.6.0 -C web screenshots -- --states --out /local/celeris/data/workspaces/01M44XGC0TDNPHQCPQYT9VZA6Q/wu/shots/artifacts/after-states`
  - 出力: `screenshots: 116 image(s) across 8 states`
- before（120 枚、`/notifications` 追加前の台帳）: `/local/celeris/data/workspaces/01M3YEN6B6AWRGRVGTPKGQ32YP/wu/before-shots/artifacts/before`

## after と before の対応表

file 名の同名対応（31 名前 × 4 幅 = 124 行）。before 有無・after 有無は同名 file の存在。

- before に無い名前（after のみ）: `_notifications-*`（4 幅）。`/notifications`（通知）は before 撮影後に台帳へ追加された画面（`docs/frontend/UX_AUDIT.md` §撮影 の before は 120 枚・30 画面）。
- after に無い名前（before のみ）: なし。before の 120 枚は全て after に同名存在。

| 名前 | 幅 | before 有無 | after 有無 |
|---|---|---|---|
| _-360.png | 360px | 有 | 有 |
| _-390.png | 390px | 有 | 有 |
| _-412.png | 412px | 有 | 有 |
| _-1440.png | 1440px | 有 | 有 |
| _accounts-360.png | 360px | 有 | 有 |
| _accounts-390.png | 390px | 有 | 有 |
| _accounts-412.png | 412px | 有 | 有 |
| _accounts-1440.png | 1440px | 有 | 有 |
| _approvals-360.png | 360px | 有 | 有 |
| _approvals-390.png | 390px | 有 | 有 |
| _approvals-412.png | 412px | 有 | 有 |
| _approvals-1440.png | 1440px | 有 | 有 |
| _artifacts-360.png | 360px | 有 | 有 |
| _artifacts-390.png | 390px | 有 | 有 |
| _artifacts-412.png | 412px | 有 | 有 |
| _artifacts-1440.png | 1440px | 有 | 有 |
| _board-360.png | 360px | 有 | 有 |
| _board-390.png | 390px | 有 | 有 |
| _board-412.png | 412px | 有 | 有 |
| _board-1440.png | 1440px | 有 | 有 |
| _clusters-360.png | 360px | 有 | 有 |
| _clusters-390.png | 390px | 有 | 有 |
| _clusters-412.png | 412px | 有 | 有 |
| _clusters-1440.png | 1440px | 有 | 有 |
| _daemon-360.png | 360px | 有 | 有 |
| _daemon-390.png | 390px | 有 | 有 |
| _daemon-412.png | 412px | 有 | 有 |
| _daemon-1440.png | 1440px | 有 | 有 |
| _graph-360.png | 360px | 有 | 有 |
| _graph-390.png | 390px | 有 | 有 |
| _graph-412.png | 412px | 有 | 有 |
| _graph-1440.png | 1440px | 有 | 有 |
| _help-360.png | 360px | 有 | 有 |
| _help-390.png | 390px | 有 | 有 |
| _help-412.png | 412px | 有 | 有 |
| _help-1440.png | 1440px | 有 | 有 |
| _inbox-360.png | 360px | 有 | 有 |
| _inbox-390.png | 390px | 有 | 有 |
| _inbox-412.png | 412px | 有 | 有 |
| _inbox-1440.png | 1440px | 有 | 有 |
| _knowledge-360.png | 360px | 有 | 有 |
| _knowledge-390.png | 390px | 有 | 有 |
| _knowledge-412.png | 412px | 有 | 有 |
| _knowledge-1440.png | 1440px | 有 | 有 |
| _knowledge_inbox-360.png | 360px | 有 | 有 |
| _knowledge_inbox-390.png | 390px | 有 | 有 |
| _knowledge_inbox-412.png | 412px | 有 | 有 |
| _knowledge_inbox-1440.png | 1440px | 有 | 有 |
| _knowledge_skills-360.png | 360px | 有 | 有 |
| _knowledge_skills-390.png | 390px | 有 | 有 |
| _knowledge_skills-412.png | 412px | 有 | 有 |
| _knowledge_skills-1440.png | 1440px | 有 | 有 |
| _login-360.png | 360px | 有 | 有 |
| _login-390.png | 390px | 有 | 有 |
| _login-412.png | 412px | 有 | 有 |
| _login-1440.png | 1440px | 有 | 有 |
| _notifications-360.png | 360px | 無 | 有 |
| _notifications-390.png | 390px | 無 | 有 |
| _notifications-412.png | 412px | 無 | 有 |
| _notifications-1440.png | 1440px | 無 | 有 |
| _org-360.png | 360px | 有 | 有 |
| _org-390.png | 390px | 有 | 有 |
| _org-412.png | 412px | 有 | 有 |
| _org-1440.png | 1440px | 有 | 有 |
| _org_cos-360.png | 360px | 有 | 有 |
| _org_cos-390.png | 390px | 有 | 有 |
| _org_cos-412.png | 412px | 有 | 有 |
| _org_cos-1440.png | 1440px | 有 | 有 |
| _plans_new-360.png | 360px | 有 | 有 |
| _plans_new-390.png | 390px | 有 | 有 |
| _plans_new-412.png | 412px | 有 | 有 |
| _plans_new-1440.png | 1440px | 有 | 有 |
| _projects-360.png | 360px | 有 | 有 |
| _projects-390.png | 390px | 有 | 有 |
| _projects-412.png | 412px | 有 | 有 |
| _projects-1440.png | 1440px | 有 | 有 |
| _projects_P1-360.png | 360px | 有 | 有 |
| _projects_P1-390.png | 390px | 有 | 有 |
| _projects_P1-412.png | 412px | 有 | 有 |
| _projects_P1-1440.png | 1440px | 有 | 有 |
| _projects_P1_docs-360.png | 360px | 有 | 有 |
| _projects_P1_docs-390.png | 390px | 有 | 有 |
| _projects_P1_docs-412.png | 412px | 有 | 有 |
| _projects_P1_docs-1440.png | 1440px | 有 | 有 |
| _projects_P1_docs_maintenance-360.png | 360px | 有 | 有 |
| _projects_P1_docs_maintenance-390.png | 390px | 有 | 有 |
| _projects_P1_docs_maintenance-412.png | 412px | 有 | 有 |
| _projects_P1_docs_maintenance-1440.png | 1440px | 有 | 有 |
| _providers-360.png | 360px | 有 | 有 |
| _providers-390.png | 390px | 有 | 有 |
| _providers-412.png | 412px | 有 | 有 |
| _providers-1440.png | 1440px | 有 | 有 |
| _releases-360.png | 360px | 有 | 有 |
| _releases-390.png | 390px | 有 | 有 |
| _releases-412.png | 412px | 有 | 有 |
| _releases-1440.png | 1440px | 有 | 有 |
| _reports-360.png | 360px | 有 | 有 |
| _reports-390.png | 390px | 有 | 有 |
| _reports-412.png | 412px | 有 | 有 |
| _reports-1440.png | 1440px | 有 | 有 |
| _tasks-360.png | 360px | 有 | 有 |
| _tasks-390.png | 390px | 有 | 有 |
| _tasks-412.png | 412px | 有 | 有 |
| _tasks-1440.png | 1440px | 有 | 有 |
| _tasks_T1-360.png | 360px | 有 | 有 |
| _tasks_T1-390.png | 390px | 有 | 有 |
| _tasks_T1-412.png | 412px | 有 | 有 |
| _tasks_T1-1440.png | 1440px | 有 | 有 |
| _tasks_T1_changes-360.png | 360px | 有 | 有 |
| _tasks_T1_changes-390.png | 390px | 有 | 有 |
| _tasks_T1_changes-412.png | 412px | 有 | 有 |
| _tasks_T1_changes-1440.png | 1440px | 有 | 有 |
| _tasks_T1_files-360.png | 360px | 有 | 有 |
| _tasks_T1_files-390.png | 390px | 有 | 有 |
| _tasks_T1_files-412.png | 412px | 有 | 有 |
| _tasks_T1_files-1440.png | 1440px | 有 | 有 |
| _tasks_T1_runs_R1-360.png | 360px | 有 | 有 |
| _tasks_T1_runs_R1-390.png | 390px | 有 | 有 |
| _tasks_T1_runs_R1-412.png | 412px | 有 | 有 |
| _tasks_T1_runs_R1-1440.png | 1440px | 有 | 有 |
| _tasks_new-360.png | 360px | 有 | 有 |
| _tasks_new-390.png | 390px | 有 | 有 |
| _tasks_new-412.png | 412px | 有 | 有 |
| _tasks_new-1440.png | 1440px | 有 | 有 |
## 未解決事項・提案

- lint warning 5 件（既存）は直していない。`states.spec.ts` の未使用引数（`checks` の `url`）と `styles.css` の `!important` 4 件は別の小 WU で扱える。
- after screenshot と before の同名対応表は `shots` WU（本 run）が上記「after と before の対応表」に書いた。
- `test.fail` の仕組み（`states.spec.ts` の `findings`）は残してある。所見を書けば、その画面は再び期待どおり失敗として数えられる。
