---
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
status: done
completed: 2026-10-05
adr: agent-docs/adr/2026-10-05-browser-department-web-live-view.md
unit: web-ui
---

# web-ui: browser 画面（run 一覧・Live View・lease・待ち・identity）と導線

## 要約

- `web/features/browser/` に browser の画面を作った（run 一覧 `/browser`、run 画面 `/browser/runs/$taskId/$runId`、本人情報 `/projects/$id/browser-identities`）。
- gateway は `web/server/browser-live.js` の proxy をそのまま使う（変更なし）。Live View は同一 origin の `/browser/live/{task_id}/{run_id}` だけで、raw `live_view_url` は DOM にも JSON にも出ない。
- 導線: nav に「ブラウザ」1 行、task 詳細の browser 節、受信箱・承認の browser_wait 項目から run 画面・`#browser-waits` へ。
- 狭幅: 360px の run 一覧は状態を links より先に出す。run 画面は control bar を上端に固定し、Live View・待ち・イベントの順に積む。
- 共有の修正 1 件: `web/components/shell/screen-frame.tsx` の breadcrumb の link を `min-w-11 px-1` にした（後述「変更」）。
- `crates/`・`gui/`・`web/server/browser-live.js` は変更していない。

## 旧 GUI との対応表（ADR D3.7 の『同等以上』の突き合わせ）

| 旧 GUI | web 側の file | 試験名（web/e2e/browser/ ほか） | 差・判定 |
|---|---|---|---|
| `components/BrowserRunsPanel.tsx`（task overview の「ブラウザ」card） | `features/browser/task-browser-section.tsx`＋`features/browser/browser-runs-screen.tsx`（`/browser` 一覧） | `runs-list.spec.ts`「owner sees T1 runs with waits and lease badges and opens the run screen」／`links.spec.ts`「…task 詳細から Live View と待ちの回答へ進める」 | 同等以上。全体の一覧を新設し、待ちの数・lease 状態を一覧に出す |
| `BrowserRunsPanel` の `safeLivePath`・`DISABLED_TEXT` | `features/browser/live-view-frame.tsx`・`browser-model.ts`（理由 7 種） | `proxy-authz.spec.ts`「a run under another task and a missing run fail closed for live and control」／単体 `browser-model.test.ts`（理由の文言・href の検査） | 同等以上。理由に `grant_expired` を追加 |
| `routes/browser.live.ts`＋`celeris/browser-live.server.ts`＋`browser-live-relay.server.ts` | `web/server/browser-live.js`（gateway 単体 `browser-live.test.mjs`） | `proxy-authz.spec.ts`「live and control reject a different cookie session and a session without owner grant」・「generic API relay rejects browser mutations and removes raw live URLs from JSON and SSE」 | 同等以上。grant を更新する（旧は 60 秒で切れた）。lease holder の入力だけ転送（単体 `WebSocket stream forwards input only while this owner holds the lease`） |
| `browser-live.server.ts` の injected 「Live events」aside | `features/browser/live-events.tsx`（SPA 側で `/read` を表示） | `narrow-a11y.spec.ts`「360px run puts the sticky control bar before Live View, waits and events」 | 同等以上。dashboard の HTML を書き換えず、upstream が無くても監視できる |
| `routes/browser.owner-session.ts`＋`browser-owner.server.ts` | `POST /browser/owner-session`（`web/server/browser-live.js`）＋`features/browser/owner-session-notice.tsx` | `identities.spec.ts`「a non-owner session sees the owner-session notice and no identities」・`runs-list.spec.ts`「a non-owner session sees the owner-session notice instead of the list」 | 同等 |
| `browser-attestation.server.ts` | `web/server/browser-live.js` 内の attestation（同じ鍵 file） | 単体 `wait decision and credential require owner CSRF and omit credentials from reply`（`browser-live.test.mjs`） | 同等 |
| `routes/browser.control.ts`＋`components/BrowserControl.tsx` | `/browser/control/*`（gateway）＋`features/browser/control-bar.tsx` | `lease.spec.ts`「owner takes over, renews, resumes, and leaving the screen releases the lease」・「pagehide releases the lease and an expired lease shows paused」 | 同等以上。「監視のみ／操作中」の明示、返却の 3 層、POST 後に状態を取り直す |
| `routes/browser.waits.$waitId.decision.ts`／`credential.ts`＋`BrowserWaitsPanel.tsx` | `/browser/waits/*`（gateway）＋`features/browser/browser-waits-panel.tsx` | `waits.spec.ts`「decision approve_once／deny reaches the fixture and closes the wait」・「credential reaches the fixture while the response and screen forget the password」 | 同等。受信箱の badge から直接辿れる |
| `routes/approvals.tsx`・`routes/inbox.tsx` の読み取り一覧 | `web/features/approvals`・`web/features/inbox` の badge と link | `links.spec.ts`「承認画面にも browser wait の実行画面への導線がある」・「受信箱の browser_wait から run へ…」 | 同等以上。link 先は run 画面 |
| `routes/browser.identities.$projectId.tsx` | `features/browser/identities-screen.tsx`（`/projects/$id/browser-identities`） | `identities.spec.ts`「owner lists, registers, restores, revokes and deletes identities of a project」・「the project detail links to its browser identities」 | 同等以上。restore を追加（旧 GUI に無い） |
| `celeris/task-detail.server.ts` の `redactLiveViewUrls`、`routes/events.ts` の SSE redact | `web/server/relay.js`・`web/server/events.js` の redact | 単体 `relay.test.mjs`（`[redacted]`）・`events.test.mjs`／`proxy-authz.spec.ts`「…removes raw live URLs from JSON and SSE」 | 同等以上。org profile など全 JSON に広げた |

## 各葉の要約（並列 WorkUnit の結果）

- fixture: 偽 browser backend（`web/e2e/support/fake-daemon.mjs` の `createBrowserBackend`）と `web/e2e/support/browser-gateway.ts`。
- query: `browser-query.ts`・表示 model・route の骨組み・nav 1 行。
- waits: 待ちの decision・credential form と本人登録の案内。
- identities: project ごとの identity 画面と project からの link。
- run-screen: run 画面（Live View・control bar・lease・返却・イベント）。
- runs-screen: `/browser` の run 一覧と未決の待ち。
- authz-e2e: proxy 認可・待ち回答・狭幅 a11y の Playwright 試験（`web/e2e/browser/`）。
- links: task 詳細・受信箱・承認からの導線と待ち badge。
- close（この葉）: 全検査・mobile-audit の対象追加・共有 breadcrumb の修正・本記録。

## 変更（close の葉で足したもの）

1. `web/e2e/support/screens.ts`: 3 行を足した（`/projects/$id/browser-identities`、`/browser`、`/browser/runs/$taskId/$runId`）。`check:parity` は gateway の SPA route 一覧と台帳の一致を見るので、この 3 行が無いと落ちていた（`missing V3 screen`）。`v3: true` は付けていない。付けると S1 latency・S2 realtime refetch の掃引（`v3Screens()` を使う）にも入り、それぞれの試験が browser backend 付きの fixture を前提にしてしまうため。`/browser` の heading は nav の名前と同じ「ブラウザ」にした（axe.spec.ts が nav から遷移する）。
2. `web/components/shell/screen-frame.tsx`: breadcrumb の link（非末尾の段）に `min-w-11 justify-center px-1` を足した。before: `/projects/P1/browser-identities` @360〜1440 で「案件」の link が 28×44 px（44×44 未満で mobile-audit が落ちる）。after: 4 幅とも通過。短い段の label（2 字の「案件」）で幅が足りなくなる構造の問題で、他の breadcrumb（`run-log-view.tsx`、label「task T1」）は長さで偶然通っていた。

## 証拠コマンドと結果

すべて `web/` で実行（exit 0 の物は exit 0）。

- `pnpm typecheck`（`tsc -b`）: exit 0。
- `pnpm lint`（`biome check .`）: exit 0（`Checked 355 files. Found 5 warnings.`。warning は styles.css の `scroll-behavior: auto !important`、既存）。
- `pnpm test`（`vitest run && node --test server/*.test.mjs`）: vitest `Test Files 66 passed (66)`、`Tests 428 passed (428)`。node:test `tests 52 pass 52 fail 0`。
- `pnpm check:boundaries`: exit 0。
- `pnpm check:parity`: exit 0（出力なし。修正前は `/projects/$id/browser-identities`・`/browser`・`/browser/runs/$taskId/$runId`: missing V3 screen）。
- `pnpm check:secrets`: exit 0（`token absent from build output, HTML, /api responses, errors and logs`）。
- `pnpm build`: exit 0（`dist/index.html` `dist/assets/index-*.js` 803.94 kB、gzip 236.37 kB）。
- `WEB_E2E_SCOPE=functional pnpm exec playwright test --grep-invert "空白帯が無く.*360x800|stale の / [0-9]+: scroll 前|\(5\) / 360: 会話枠の先頭に見える"`: `220 passed`、`8 skipped`、failed 0（`--list` で 228 件・除外は 3 件分の一致を確認）。
- `WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/work/narrow-r6.spec.ts --workers 1`: `14 passed`（`(5) / 360` を含む。負荷で揺れる 1 件は単独では通る）。
- `WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/browser`: `19 passed`。
- `node scripts/mobile-audit.mjs --screenshots <artifacts>/mobile-audit-close`: `mobile-audit: 34 path(s) x 4 widths ok`、exit 0。スクリーンショット 136 枚（artifacts の `mobile-audit-close/`。browser 3 画面は `_browser-<w>.png`・`_browser_runs_T1_R1-<w>.png`・`_projects_P1_browser_identities-<w>.png`、w = 360/390/412/1440）。

## 未解決事項

- mobile-audit の fixture（`scripts/fixture-gateway.mjs`）は password を設定せず owner socket も無いので、browser 画面は「本人確認を取得できません／利用できません」の状態で監査される。owner 状態の control bar・待ち form の 44 px は、この監査では見ていない。owner 状態の狭幅は `e2e/browser/narrow-a11y.spec.ts`（360px）が確かめる。
- 既知の失敗（main 由来、この葉では直していない）: `e2e/shell/home-layout.spec.ts:26`（360x800）と `e2e/work/home-stale-viewport.spec.ts`（stale の / 146<150px）。除外して確かめた。このブランチで除外なしの再現はしていない（memory の main 8dc4c5e4 での観測）。
- 負荷で揺れる `e2e/work/narrow-r6.spec.ts:173`（`(5) / 360`）は全体実行から除外。単独（`--workers 1`）では 14 件とも通過。
- S1 latency・S2 realtime の nfr scope は、この run では流していない（`WEB_E2E_SCOPE=functional` のみ）。
- 旧 GUI（`gui/`）の撤去は本 ADR の範囲外。

## 提案

- `Breadcrumb` の link を `min-w-11` にしたのと同じ理由で、他の shared の短い label（タブ・chip）も 44 px 幅を確かめる。mobile-audit は自動で拾うので、台帳に screen を足すたびに実行する。
- owner 状態の mobile 監査を足すなら、`scripts/mobile-audit.mjs` に `browser-gateway` 相当（owner 承認付き）の起動を入れる。この run では作らなかった（試験基盤の拡張が要るため）。
- `v3: true` を browser 画面に付けるかは、S1・S2 の browser backend 対応を決めてから（別の決定）。
