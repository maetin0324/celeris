---
title: 共通部品の a11y critique と修正（focus-visible・reduced motion・contrast・keyboard）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-04
---
# 共通部品の a11y critique と修正

WorkUnit a11y の記録。対象は web/components/（ui・shell・fetch-state・actions・content）・web/styles.css・
web/routes/__root.tsx。差分基点は 7eab1be6。部品の追加（Notice・Select/Input・ShortId・tab 端 fade）は
並行の WorkUnit primitives が担当し、この記録の範囲外。

## critique（ui-ux-quality-gate による横断の見立て）

- 面の種類: ops workbench（Celeris の task・組織・運用を一人の運用者が見る）。主の操作は keyboard と pointer の両方、
  スマホでは「メニュー」から辿る。
- keyboard
  - tab 順: DOM 順と見た目の順は一致（header → nav → main → Console）。正の tabIndex は無い（mobile-audit で確認）。
  - skip link が無かった。header と nav（20 項目近い link）を毎画面 Tab で越える必要があった。→ 直した。
  - Esc: Drawer・ConfirmDialog は Radix Dialog/AlertDialog で Esc に閉じ focus を戻す。shell の「メニュー」は
    独自に Esc で閉じて「メニュー」へ focus を戻す。問題なし。
  - roving: 共通部品に tablist・menu・grid のような複合 widget は無い（nav は link の list で、Tab で辿る方が正しい）。
    要る所は無い。tab 部品が primitives で足されたら、そちらで roving を確かめる（残課題）。
- :focus-visible
  - 遷移後の h1 は `focus:outline-none` で、pointer でも keyboard でも枠が出なかった。Tailwind v4 の `outline-none` は
    `--tw-outline-style: none` も固定するので、`focus-visible:outline-2` を足しても keyboard の枠は none のままになる
    （e2e で実測）。→ `focus:outline-none` を外し、`focus-visible:outline-2 outline-offset-2 outline-ring` だけにした。
    遷移後の h1 への focus 移動（S4）は shell.tsx のまま保つ。Chromium は pointer 起点の script focus を
    focus-visible にしないので pointer では枠が出ず、keyboard 起点では 2px の ring が出る。
  - Button・Drawer の閉じる button は既に focus-visible の ring。
- contrast（computed の値から算出）
  - 本文 #20303C / 白 15:1 以上、補足 #536572 / 白 6.05:1、/ 地 #F4F7F9 5.62:1。主色 #245675 / 白 7.89:1。
    status の前景・背景の組は 6.6〜8.0:1。いずれも 4.5:1 を満たす。
  - 入力欄の枠 --color-input #758593 / 白 3.80:1、/ 地 #F4F7F9 3.53:1。3:1（WCAG 1.4.11）を満たす。
  - placeholder が Tailwind 既定（本文色の半透明）で白地 3:1 前後だった。→ 補足色（6.05:1）に揃えた。
  - 無効状態: Button の disabled は #475569 / #F1F5F9 6.92:1。入力欄の disabled は地の色が変わらず区別しにくかった。
    → 地を --color-muted にし cursor を not-allowed にした（文字は本文色のまま、枠は --color-input のまま）。
- touch target 44px: Button・nav link・パンくず・Drawer の閉じるは min-h-11。新しい skip link も隠れている間を含め
  44px を保つ（sr-only の 1px は mobile-audit が違反にするため、画面外への translate で隠す）。
- prefers-reduced-motion: styles.css の全要素に transition・animation 0.01ms と scroll-behavior auto が既にあった。
  0 にしない理由（Radix Presence が animationend を待つ）を注記した。Drawer は motion-reduce:transition-none も持つ。

## 修正

| file | 変更 |
|---|---|
| web/components/shell/screen-frame.tsx | h1 の `focus:outline-none` を外し focus-visible の ring のみに |
| web/components/shell/shell.tsx | skip link「本文へ移動」（素の `#main` link・focus 時だけ表示・44px）、main に tabIndex=-1 と focus:outline-none |
| web/styles.css | ::placeholder を補足色に、無効な入力欄の地、reduced motion の注記 |
| web/e2e/a11y/focus-motion.spec.ts | 新規（functional。playwright.config.ts の NFR 一覧には入れていない） |

e2e の内容: (a) pointer 遷移後の h1 は outline-style none、keyboard 遷移後は solid 2px 以上。skip link は header の
最初の link の前にあり、Enter で main へ focus（path は変わらない）。(b) reducedMotion: 'reduce' で #root 以下の全要素の
transition・animation duration が 1ms 以下、既定では 50ms を超える要素がある。(c) /login・/knowledge・/tasks/new の
入力欄の枠色とその背景の contrast を computed style から算出して 3:1 以上。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` exit 0
- `pnpm -C web typecheck` exit 0 / `lint` exit 0（warning 4 は既存の reduced motion の !important）/ `test` exit 0
- `pnpm -C web check:boundaries`・`check:parity`・`build`・`check:secrets` exit 0
- `pnpm -C web mobile-audit` → `mobile-audit: 31 path(s) x 4 widths ok`
- `pnpm -C web e2e e2e/a11y/focus-motion.spec.ts` → 6 passed
- `pnpm -C web e2e`（functional 全体）→ 133 passed, 8 skipped
- `WEB_E2E_SCOPE=nfr playwright test e2e/a11y/axe.spec.ts` → 32 passed（skip link を足しても axe serious/critical 0）
- screenshot: run の artifacts の screenshots/before・after（360/390/412/1440 で pointer 遷移・keyboard 遷移・
  skip link・/login）

## 残課題

- features/ の画面が独自に持つ h1（例 features/tasks/task-list-view.tsx）は対象外。共通の ScreenFrame へ寄せると
  focus 枠の扱いが揃う。
- 横 scroll の tab・Select・Notice は primitives の WorkUnit で足される。tablist の roving（矢印キー）と 44px は
  統合後の検査で確かめる。
- skip link の行き先は main。Chromium 以外の focus-visible の heuristic（Safari の script focus）は未確認。

## 提案

- FRONTEND_CONTRACT.md に「`focus:outline-none` と `focus-visible:outline-*` を同じ要素に併記しない
  （Tailwind v4 では --tw-outline-style が none に固定される）」を足す。
