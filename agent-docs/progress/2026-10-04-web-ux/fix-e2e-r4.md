---
title: visual QA functional e2e の退行修正 r4
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# visual QA functional e2e の退行修正 r4

## 修正前の切り分けと修正

| 失敗群 | 修正前の失敗原因 | 修正 |
| --- | --- | --- |
| `parity/task-detail.spec.ts` 105・163・291 行 | **spec の古さ**。判断は `done` / `reviewing`、実行段階は `awaiting_human` / `executing` を本文に期待していたが、画面は和名「完了」「レビュー中」「人の判断待ち」「実行中」を表示する。操作・再取得は動作していた。 | 判断は既存の `data-status`、実行は既存の `data-phase` で raw 値を確認。和名表示は判断の「完了」を 1 本で確認。判断操作・409・実行確定の assert は維持。 |
| `parity/console.spec.ts` 315 行 | **spec の古さ**。ホームの会話が `data-home-console` の独立 scroll 領域になった後も、試験は `window.scrollTo` で会話から離れ、window の末尾距離を測っていた。そのため「最新へ」が出なかった。 | 会話枠（`data-home-console`）の末尾距離を、枠が意図して残す送信欄の逃げ（console-region.tsx の slack と同じ式）を除いて従来どおり 24px 以内で確認。枠を上へ動かして追記後に動かないこと、「最新へ」で追記が viewport に入ること、末尾で追記を追うこと、window の scroll 0 を確認。 |
| `shell/home-layout.spec.ts` 360x800 | **画面の退行**。修正前は `window.scrollY=60`、document が viewport より 60px 高かった。ConsoleView の旧 window 追従が初回にその 60px を scroll し、さらに会話枠の最低高 240px が残り高さ約 180px を超えた。after 画像では判断項目の題も細い列に圧縮されていた。 | ホームの ConsoleView では window 追従を止め、会話枠自身が追う。枠の最低高を 160px にし、360x800 に収めた。判断項目の題はスマホ幅では 1 行分の幅を確保した。h1・判断待ち・入力欄の viewport 内表示を保つ。 |
| `states/rich-data.spec.ts` 40 行 | **spec の古さ**。既定 fixture の取得は成功し、実行段階の `verifying` が「検証中」と表示されていた。旧 spec は raw 値の本文表示を期待していた。 | 既存の `data-phase="verifying"` で raw 値を確認。取得失敗 0 件、計画・routing・報告・承認の assert は維持。 |

いずれも修正前に対象 spec だけを実行して失敗を確認した。試験の skip・削除、操作・取得・表示の assert の削除はしていない。

## 検査と画面確認

- 修正後の個別検査: task-detail 7 passed、console 8 passed、home-layout 3 passed、rich-data 2 passed。
- `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'`: 最終変更後（HEAD f7042863）に再実行して 181 passed / 8 skipped / exit 0（skip は 8 のまま）。
- `corepack pnpm@12.6.0 -C web typecheck`・`lint`: exit 0。`mobile-audit`: 31 path × 4 幅 ok。`git diff --name-only 9da059a7..HEAD -- crates web/server`: 0 件。
- commit: cb4cd6ee（task-detail・rich-data spec 追従）、3b0aa868（ホーム画面の修正）、f7042863（Console spec 追従）。
- 画面の before: `01M456GWPPBTZ339CASNC3EA2T/wu/home-layout/artifacts/home-before/home-360.png`・`home-1440.png`。after: この WU の `artifacts/home-after/_-360.png`・`_-390.png`・`_-412.png`・`_-1440.png`。after は build 後、fixture gateway で撮影した。360px の題の縦積みを画面確認で発見し、題の幅を修正して撮り直した。
- UI/UX 判定: **可**。ホームの h1、期限順の判断、会話、送信欄を 360 / 390 / 412 / 1440px で読める。Celeris の判断待ちと run の関係を保ち、一般的な KPI カードの画面にはしていない。見た目の変更はホームの判断項目の配置のみ。
