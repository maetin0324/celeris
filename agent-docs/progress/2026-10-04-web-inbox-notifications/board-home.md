---
tasks: [01M43690B86J9GHP87CARYM8RS]
unit: board-home
status: done
completed: 2026-10-04
---
# board-home: board を list/table へ、home と /plans/new の改修

設計は [ADR 2026-10-04-web-inbox-notifications-screens](../../adr/2026-10-04-web-inbox-notifications-screens.md) の D5。差分の基点は 171c8e02。

## 変えたもの

- `/board`（`web/features/projects/board-screen.tsx`・`board-model.ts`・`web/routes/board.tsx`）: 6 列の card wall をやめ、
  `Table` の 1 行 1 task にした。状態ごとの束は `tbody[data-board-column]` と見出し行（件数つき）、状態は `StatusBadge`。
  表の上に件数つきの状態絞り込み（URL の `column`。daemon には渡さない表示だけの絞り込み）。編集は行の「編集」
  （`aria-expanded`）で開く行内 form（優先度・レベル・担当、保存・閉じる）で、カードへの常設をやめた。
  md 未満では優先度・レベル・担当・種類の列を題名の下の 1 行へ、sm 未満では状態も題名の下へ畳み、390px でも表もページも横に溢れない。
  絞り込み form は 案件・検索 を常設、他は「条件を足す」の details（値があれば開く）。入力欄の枠は `border-input`。
- home（`web/routes/index.tsx`）: Console の上に「受信箱 判断待ち N 件」「通知 未読 N 件」の 2 つの入口。件数は shell の常駐 query と
  同じ key の cache を `enabled: false` で読むだけで、home の初期表示に request を足さない。通知の件数は notifications 葉が shell に
  `unreadCountQuery` を足すと出る（それまでは「通知」だけ）。
- `/plans/new`（`web/routes/plans.new.tsx`）: 画面を route に移し（features/tasks は別葉の範囲）、`max-w-form`、枠は `border-input`、
  説明を `aria-describedby`、422 で `aria-invalid` と枠の danger 色・欄の下の alert。送る本文・h1・accessible name は従来どおり。
- `web/e2e/parity/projects.spec.ts` の board の試験（題名は保つ）: 表 1 つ・横に溢れない・状態絞り込みの URL・行の「編集」から保存、に合わせた。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` | exit 0 / 0（既存 warning 4 件のみ）/ 0（vitest 44 files 286 tests） |
| `check:parity`・`check:boundaries` | exit 0 |
| `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /'` | 175 passed / 8 skipped（board 修正の前の版。差分は表示 class だけ） |
| `e2e e2e/latency/transition.spec.ts --retries=2` | 30 passed |
| `e2e parity/projects・tasks・console + a11y`（最終版） | 49 passed |
| `mobile-audit`（最終版） | 30 path × 4 幅 ok |
| `screenshots --out <run artifacts>/after-board-home` | 31 画面 × 4 幅 = 120 枚。データ入りの board（一覧・編集を開いた状態・状態絞り込み）、home、/plans/new の 422 は `after-board-home-data/`（20 枚） |
| `git diff --stat 171c8e02 HEAD -- crates gui docs/api web/api/generated web/server` | 差分なし |

## 未解決事項

- home の通知入口は `/notifications` を指す（notif-route の決定 a を前提）。決定が b（`/inbox?view=notifications`）なら
  `web/routes/index.tsx` の `NOTIFICATIONS_HREF` を変える。route が未作成のため Link でなく history.push の a 要素にした。
- screenshots.mjs の `/board` fixture は tasks の取得が失敗表示になる（以前から）。データ入りの姿は `after-board-home-data/` を見る。
- `features/tasks/create-screen.tsx` の `PlanCreateScreen` は使われなくなった（別葉の範囲なので残した）。

## 提案

- verify 葉で、notif-route の決定に合わせて home の通知入口を Link に戻し、home の入口の e2e（件数の表示）を足す。
