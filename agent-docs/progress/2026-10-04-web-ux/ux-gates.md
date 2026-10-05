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

## 未解決事項・提案

- lint warning 5 件（既存）は直していない。`states.spec.ts` の未使用引数（`checks` の `url`）と `styles.css` の `!important` 4 件は別の小 WU で扱える。
- after screenshot と before との同名対応表は、この WU の範囲外（`shots` WU）。このファイルには書いていない。
- `test.fail` の仕組み（`states.spec.ts` の `findings`）は残してある。所見を書けば、その画面は再び期待どおり失敗として数えられる。
