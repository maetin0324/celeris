---
title: screenshots --states の error 待ちとホーム 1440 の空枠（fix-r5 / fix-home-states）
tasks: [01M45PRPACBEP4X1FWPNP4FRMG]
status: done
updated: 2026-10-05
---

# screenshots --states の error 待ちとホーム 1440 の空枠（fix-home-states）

## (c) --states の error が読み込み中のまま撮られる

- 原因: `web/scripts/screenshots.mjs` の --states は `page.goto` の直後に何も待たずに撮っていた。loading 用に「保留のまま撮る」書き方を全状態に使っていたため、error（偽 daemon の 503）は応答と描画の前に撮られ、`[data-fetch-state="error"][role="alert"]`（取得失敗の文と「再試行」button）が写らなかった。
- 修正: `web/e2e/support/states.ts` の `FixtureState` に `capture?: "held" | "alert" | "settled"` を足し、loading は `held`、error は `alert`、他は既定 `settled`。`waitForStateCapture(page, state)` が h1 の後に状態ごとに待つ（held: `[data-fetch-state="loading"]` が見えたら保留のまま撮る／alert: `role=alert` の中の「再試行」button が見えるまで／settled: 読み込み中の表示が消えるまで）。固定の時間では待たない。screenshots.mjs はこれを呼ぶ。
- 確認: `node scripts/screenshots.mjs --states --out <WU artifacts>/states-TUY0` → 116 枚・exit 0。`error-_tasks-390.png`（「取得に失敗しました。…再取得してください。」と「再試行」）、`error-_providers-1440.png`（「プロバイダを取得できません。…」と「再試行」）を目で見た。`loading-_tasks-360.png` は従来どおり skeleton（保留）で写る。

## (d) ホーム 1440 の通知の右に浮く空の枠の下辺

- DOM で確かめた（fixture gateway・Chromium・1440x800、`elementFromPoint` と bounding box）: 会話の枠 `div[data-home-console]` は top=349・height=425 で、中の `div[data-console]`（ConsoleView、`pb-40`）は top=318・height=547。つまり枠が末尾へ 31px 送られている（ConsoleRegion の末尾追従、会話 3 block でも送信欄の逃げの余白で溢れる）。`elementFromPoint(1400,355)` は `BUTTON「新しい会話」`（box 1302,318,114x44、border 1px）。宛先と「新しい会話」の行が枠の上端で切れ、button の下 13px（下辺の線）だけが枠の中に残って空の枠に見えていた。
- 修正: `web/features/console/console-view.tsx` の宛先・「新しい会話」の行（`data-console-toolbar`）を contained（ホームの枠）のときだけ `sticky top-0 z-10 bg-surface pb-2` にした。枠が末尾へ送られても行は枠の上端に留まり、button は切れない（修正後 button top=349 = 枠の上端）。/console・/org/:id（contained でない）は変わらない。
- 固定: `web/e2e/shell/home-layout.spec.ts` に「ホーム 1440: 会話枠の上端の右に中身の無い枠（border だけの切れた要素）が無い」を追加。x 1300〜1415・y 340〜360 の点ごとに最寄りの border 付き要素を探し、枠の上端で切れていないこと・文字を持つことを assert。修正前の build では `BUTTON「新しい会話」 top=318 枠の上端=349` で失敗し、修正後に通ることを確かめた。

## 証拠

- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` / `build`: いずれも exit 0
- `corepack pnpm@12.6.0 -C web e2e --retries=0`: exit 0（182 passed / 8 skipped）
- `corepack pnpm@12.6.0 -C web mobile-audit`: exit 0（31 path × 4 幅 ok）

## 未解決・提案

- 無し。after-r4 の撮影は shots-r4 が行う。
