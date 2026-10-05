---
title: web-ui run-screen — Live View・control bar・lease・返却・イベント
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# web-ui run-screen — Live View・control bar・lease・返却・イベント

WorkUnit `run-screen`。[ADR 2026-10-05-browser-department-web-live-view](../../../adr/2026-10-05-browser-department-web-live-view.md) の D2.4・D3.2・D3.3・D3.6 に従い、
`/browser/runs/$taskId/$runId` の中身を作った。gui/・crates/・web/server/ は変えていない。

## 変更点

- `web/features/browser/control-bar.tsx`（新規）: 上端 sticky の control bar。
  - 先頭に phase の文言（D3.3 の 6 状態、`controlPhaseText`）。`human_control` の間は `border-warning-foreground bg-warning` の枠と `data-operating="true"`。
  - 読み上げは別の `role="status" aria-live="polite"`（sr-only）。残り秒は 15 秒刻みに丸めた文（`controlAnnouncement`）なので 15 秒ごとにだけ変わる。
  - pause / takeover 60s / renew 60s（残り 15 秒以下で primary・ring・残り秒の表示）/ resume（fresh snapshot と origin 確認の 2 checkbox 必須）/ stop（ConfirmDialog）。
    有効/無効は `controlButtons`（phase・自分の lease・本人・送信中・認証区間）。送信は `sendControl`（`BrowserActionGate` で 1 度に 1 つ、UUID の idempotency key）。
  - 2 秒 poll（`browserControlQuery`）。SSE の `browser_updated` は既存の invalidation で取り直す。
  - 返却の 3 層: resume／route 離脱（unmount）と `pagehide` で `/browser/control/{task}/{run}/{session}/release` を `navigator.sendBeacon`（CSRF は form 値）／
    期限切れで `paused` を表示し「期限が切れたため一時停止しました。エージェントは自動では再開しません」と出す。
  - 「自分の lease か」は takeover の成功時に sessionStorage に置く holder で判定する（gateway の `lease_holder` は gateway の session key で、SPA から比べられない）。
- `web/features/browser/live-view-frame.tsx`（新規）: `liveViewState`（本人→実行中→認証区間→gateway の理由→`safeLivePath`）と表示。
  iframe は `sandbox="allow-scripts allow-same-origin"`・`title="ブラウザの Live View: <task>"`・16:10（`aspect-live` token）。前に「Live View を飛ばしてイベントへ」、後に「別タブで開く」link。
  不可のときは理由の文言と「映像なし・イベントで監視中」。
- `web/features/browser/live-events.tsx`（新規）: イベントの流れ。最後に見た seq（last_seen）から `after_seq` で追い、この run の行だけを出す（上限 200 行）。
- `web/features/browser/browser-run-screen.tsx`（新規）: 本人 notice → control bar → Live View → 待ち（`#browser-waits`、既存 `BrowserWaitsList`）→ イベント。
  狭幅は縦積み、`lg` 以上は Live View と 待ち・イベント の 2 列（DOM の順は同じ）。
- `web/routes/browser.runs.$taskId.$runId.tsx`: 画面を差した（19 行）。
- `web/styles.css`: `@theme` に `--aspect-live: 16 / 10` を 1 行足した（任意値 `aspect-[16/10]` を使わないため）。
- 試験: `web/features/browser/control-bar.test.tsx`（vitest 11 本）、`web/e2e/browser/lease.spec.ts`（Playwright 2 本）。

## 実行したコマンドと結果

| コマンド | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 成功 |
| `pnpm -C web exec vitest run features/browser/control-bar.test.tsx` | 11 passed（phase 6 状態・ボタンの有効/無効・warning 枠・残り 15 秒の強調・15 秒刻みの読み上げ・liveViewState・イベントの last_seen） |
| `WEB_E2E_SCOPE=functional pnpm -C web exec playwright test e2e/browser/` | 3 passed（lease.spec.ts 2 本＋fixture-smoke） |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0（既存の warning 5 件のみ） |
| `pnpm -C web test` | exit 0。Vitest 64 files / 408 tests、gateway の node --test も pass |
| `pnpm -C web check:boundaries` / `check:secrets` | 各 exit 0 |
| `pnpm -C web check:parity` | exit 1。`/browser`・`/browser/runs/$taskId/$runId`・`/projects/$id/browser-identities` が `missing V3 screen`。3 route は base（query WU）からあり、base の `e2e/support/screens.ts` にも browser の行は 0 件 — この WU 以前からの失敗 |
| FRONTEND_CONTRACT.md の生の色・任意値 grep（変更 5 file） | 0 件 |
| 一時 spec（`web/e2e/.tmp-run-screen/`、実行後に削除）で 360/390/412/1440 を撮影 | 4 passed。360/390/412 で 状態→ボタン→iframe→待ち→イベント の縦順、横溢れなし、axe serious/critical 0（監視中・操作中の両方） |

lease.spec.ts の確かめたこと: owner が 一時停止→引き継ぐ で「あなたが操作中」・warning 枠、延長で fixture の `lease_expires_at` が延びる（偽 backend の clock を 10 秒進める。
gateway の assertion は 20 秒で切れるため）、2 checkbox の後の resume で「監視のみ」に戻り `fresh_snapshot`・`policy_origin_ok` が送られる、
nav の「ブラウザ」で離れると fixture に disconnect が 1 件届き `paused`・holder なし。iframe は `/browser/live/T1/R1` の 1 つだけで別タブ link も同じ path。
2 本目: 期限切れ（fixture の期限を過去に書き換え）で「一時停止中」と期限切れの文、`page.goto` での document 離脱（pagehide）でも disconnect が届き `paused`。

スクリーンショット: `/local/celeris/data/workspaces/01M470CJT10MV8XVFEGSMCXHYS/wu/run-screen/artifacts/before/run-<w>.png`（base の骨組み）、
`…/artifacts/after/run-monitor-<w>.png`・`run-operating-<w>.png`（w = 360/390/412/1440）。

## 未解決事項

- gateway（`web/server/browser-live.js`、範囲外）に live `/read` の中継が無い。`live-events.tsx` は task-api `/read`（status・tabs・url・console）を読めず、
  generic relay で読める task の `browser_updated`（scrub 済み）だけを出している。`GET /browser/live-events/{task}/{run}?after=` のような中継を足せば、表示を差し替えるだけで済む。
- gateway の control status の `lease_holder` は gateway の session key をそのまま返している。SPA の「自分の lease か」は sessionStorage の記憶で判定するので、
  別タブ・再ログイン後の自分の lease は「他のセッションが操作中」と出る（操作は期限・disconnect で戻る）。gateway が `lease_is_mine: boolean` を返し、key を落とすのが望ましい。
- `check:parity` の browser 3 route の台帳（`e2e/support/screens.ts`）は未登録（並行 WU と同じ file のため、close か統合で一括して足す）。
- `/browser/runs?task_id=` は本人以外に 403 なので、本人でない session では run の session id が分からず control bar は「操作状態を確認できません」になる（ADR どおり notice を出す）。

## 提案

- gateway に `/read` の中継と `lease_is_mine` を足す（authz-e2e か後続の gateway の葉）。
