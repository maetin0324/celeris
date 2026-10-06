---
title: web-ui runs-screen — /browser の run 一覧と未決の待ち
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# web-ui runs-screen — /browser の run 一覧と未決の待ち

WorkUnit `runs-screen`。[ADR 2026-10-05-browser-department-web-live-view](../../../adr/2026-10-05-browser-department-web-live-view.md) の D2.4・D3.1・D3.3 に従った。
gui/・crates/・web/server/browser-live.js・web/styles.css は変えていない。

## 変更点

- `web/features/browser/browser-runs-model.ts`（新規）: 一覧の表示 model。
  - 並び: 人の対応が要る run（返却待ち `paused`・未決の待ちあり）→ 実行中 → 終了。同じ群は run id の降順。
  - lease badge: `agent_running`/`pausing` →「監視のみ」、`human_control` →「操作中」（warning）、`paused` →「一時停止中（人の返却待ち）」（warning）、`stopped` →「停止」。終了 run は badge なし。phase が未取得なら「操作状態を確認中」、control の取得失敗なら「操作状態を取得できません」。
  - live の可否と理由: gateway が `live.state=disabled` を返せばその理由（未知の理由は `relay_unavailable` に丸める）、無ければ 終了 → `not_running`、認証待ちあり → `auth_interval`、`safeLivePath` に合わない → `not_configured`。文言は `liveUnavailableText`。
  - 待ちの数は `pending` の wait だけを run ごとに数える。
- `web/features/browser/browser-runs-screen.tsx`（新規）: `/browser` 画面。
  - owner-session を先に取り、本人でなければ一覧を取らずに `OwnerSessionNotice` を出す（gateway の `/browser/runs` は他人に 403）。
  - 本人なら `/browser/runs`（10 秒 poll）、`/api/browser/waits`（`BrowserPendingList`、generic relay で通る GET）、実行中 run ごとの control GET（10 秒）、task 題（waits の `task.title` → task 詳細）を並べる。
  - 行は `<ul>` のカード。題・task id・run id（`break-all` で折り返す）・Live View の可否・状態 badge・lease badge・待ちの数、「Live View を開く／run を開く」（`/browser/runs/$taskId/$runId`）と「task 詳細」（`/tasks/$id`）の link（`min-h-11`）。md 以上は badge を右に寄せ、360px では縦に積む。
  - 未決の待ちは `BrowserWaitsList`（browser-waits-panel）をそのまま使う。無ければ「未決の待ちはありません。」。
  - loading/error/empty は `FetchFrame`。raw `live_view_url` は扱わない（行の model にも持たない）。
- `web/routes/browser.index.tsx`: 骨組みを `BrowserRunsScreen` に差し替えた（4 行）。
- `web/features/browser/browser-runs-model.test.ts`（新規）: 並び・badge・理由・取得失敗・raw URL を持ち込まないことの 5 本。
- `web/e2e/browser/runs-list.spec.ts`（新規）: owner で T1/R1（待ち 1 件・監視のみ・実行中）と T1/R0（終了・not_running の理由・lease なし）を確かめ、run 画面と task 詳細へ遷移する。DOM（`page.content()`・全 `a[href]`/`iframe[src]`）に fixture の raw URL が無い。360px でカードが幅に収まり横溢れなし・link が 44px 以上。本人でない session は notice が出て行が 0。

## 実行したコマンドと結果

| コマンド（`web/` で、pnpm は `corepack pnpm@12.6.0`） | 結果 |
|---|---|
| `install --offline --frozen-lockfile` | 成功 |
| `exec vitest run features/browser/` | 5 files / 35 tests pass |
| `typecheck` | exit 0 |
| `lint` | exit 0（既存の warning 5 件のみ） |
| `test` | vitest 64 files / 402 tests pass、gateway node:test 52 pass / 0 fail |
| `check:boundaries` / `check:secrets` | 各 exit 0（route は 4 行） |
| `build` 後 `WEB_E2E_SCOPE=functional WEB_E2E_WORKERS=1 exec playwright test e2e/browser/` | 4 passed（fixture-smoke 1 + runs-list 3） |
| 一時台本（`e2e/.tmp-runs/`、実行後に削除）で 360/390/412/1440 を撮影し axe serious/critical を確認 | 違反 0。画像 12 枚 |
| `grep` で生の色・任意 px 値 | 該当なし |
| `git diff --name-only $CELERIS_WU_BASE` | 上記 5 file と本記録のみ |

画像: `/local/celeris/data/workspaces/01M470CJT10MV8XVFEGSMCXHYS/wu/runs-screen/artifacts/after/`（`browser-<w>.png` = credential 待ちあり、`browser-monitor-<w>.png` = 待ち W1 のみ、`browser-not-owner-<w>.png` = 本人でない session）。変更前は骨組みの 1 行（query 葉の `after/` と同じ）。

## 未解決事項

- `check:parity` は `/browser`・`/browser/runs/$taskId/$runId`・`/projects/$id/browser-identities` の「missing V3 screen」で exit 1。query 葉が route を足した時点（base）から同じで、この葉の変更とは無関係。`e2e/support/screens.ts` の台帳登録は close 葉で行う想定（query.md「後続へ」）。
- 偽 daemon に credential 待ち W2 があると、gateway（`browser-live.js` の `isAuthWait` が namespace 全体を見る）の control GET が失敗し、一覧の lease は全行「操作状態を取得できません」になる（`browser-<w>.png`）。fixture.md の提案（`isAuthWait` を開いている wait に絞る）が直れば「監視のみ」に戻る。
- gateway の `/browser/runs` は task 題も live の理由も返さない。画面は waits の `task.title` と task 詳細の query で題を補い、理由は client で導く。gateway が `live` を返すようになれば model はそれを優先する。
