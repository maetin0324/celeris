---
title: web/ の成果物をブラウザ内で表示する（旧 GUI 同等）
tasks: [01M46X1BPS53HYHP6J7TE7V6D5]
status: done
updated: 2026-10-05
completed: 2026-10-05
---
# web/ の成果物をブラウザ内で表示する（旧 GUI 同等）

ADR: [2026-10-05-web-artifact-inline-view](../adr/2026-10-05-web-artifact-inline-view.md)

## やったこと
- gateway `web/server/files.js`: 表示用 `view=1`（daemon へは送らない、`download=1` と併用は 400）。octet-stream の html/htm・svg・pdf に
  拡張子で Content-Type を補い inline。HTML・SVG 等は `CSP sandbox`（allow-* なし）+ `frame-ancestors 'self'`、PDF は sandbox を外し
  `default-src 'none'; object-src 'self'`。`view=1` の応答だけ `X-Frame-Options: SAMEORIGIN`。view 無しは従来どおり。
- `web/server/app.js`: SPA document の CSP に `frame-src 'self'`、`object-src 'self'`（埋め込み先は view=1 のみ）。
- `web/components/content/`: `artifact-kind.ts`（拡張子→種類）、`artifact-viewers.tsx`（Markdown／text・code・JSON・CSV・log の
  128 KiB 範囲取得・行番号・折り返し切替・続きを読む／画像の拡大縮小・等倍・全体／SVG は img／PDF は object＋代替案内／
  HTML は `sandbox=""` iframe）、`artifact-preview.tsx`（本文をここで見る・新しいタブで開く・ダウンロード）。
- `web/features/artifacts/artifact-table.tsx`: sha256 先頭 12 桁（全文は title）と「記録後に変更あり」。
- 試験: `web/server/files.test.mjs`（+5）、`web/components/content/artifact-preview.test.tsx`（種類・範囲取得の UTF-8 境目）、
  `web/e2e/work/artifact-viewer.spec.ts`（実 gateway＋偽 daemon、md・log・png・svg・pdf・html・script 付き html・zip、4 幅）。
  偽 daemon に `length` と `X-Celeris-Size` を足した。

## 証拠
| コマンド | 結果 |
|---|---|
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0（既存 warning 5: states.spec.ts・styles.css） |
| `pnpm -C web test` | exit 0（vitest 363 passed、node --test 47 pass） |
| `pnpm -C web check:boundaries` | exit 0 |
| `WEB_E2E_SCOPE=functional playwright test e2e/work/artifact-viewer.spec.ts` | 5 passed |
| `pnpm -C web e2e`（functional 全体） | exit 1: 208 passed / 3 failed / 8 skipped。失敗はホーム（/）の 3 件のみ。home-layout.spec.ts:26 と home-stale-viewport.spec.ts:15 は base f79a134d でも同じく失敗（worktree で確認）。narrow-r6.spec.ts:173 は単独再実行 2 回とも 14 passed（全体実行の負荷での揺れ） |
| `bash scripts/dev/test-parallel.sh` | exit 0（nextest 3942 passed / 0 failed / 13 ignored） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |

screenshot（run の artifacts）: `after/task-artifacts-open-{360,390,412,1440}.png`、`after/task-artifacts-html-pdf-{…}.png`、
`before/task-artifacts-before-{…}.png`。headless の Chromium（headless shell）は PDF viewer を持たないので、PDF は代替案内が写る。

## 未解決事項
- web の承認画面（features/approvals）は成果物の本文を出していない（旧 GUI の ApprovalArtifactPreview 相当なし）。今回は
  shell・並行 task との衝突を避けて範囲外にした。task 詳細の成果物 tab と /artifacts で同じ表示が使える。
- 実 browser の PDF viewer での表示（Chrome・Firefox・iOS Safari）は人の目視確認が要る（headless shell では確かめられない）。
- ホーム（/）の functional e2e 2 件は main で既に落ちている。

## 提案
- JS 付き HTML 報告（グラフ等）を動かしたいなら、別 origin（専用 host）からの配信と `sandbox allow-scripts` を併せて別 ADR で決める。
- daemon の Content-Type 表（api.md §3.8）に html・svg・pdf を加えるかは、gateway の補完と二重になるので不要と考える。
