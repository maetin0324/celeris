---
title: 主画面 2 — 受信箱（判断を返す）と通知（既読・束ね）の 2 入口・home・projects・board の改修
tasks: [01M43690B86J9GHP87CARYM8RS]
status: done
completed: 2026-10-04
---

# 主画面 2: 受信箱と通知の 2 入口・home・projects・board

設計は [ADR 2026-10-04-web-inbox-notifications-screens](../adr/2026-10-04-web-inbox-notifications-screens.md)。
差分の基点は 171c8e02a555。人の決定 notif-route = a（`/notifications` を新設）。葉ごとの記録は
[2026-10-04-web-inbox-notifications/](2026-10-04-web-inbox-notifications/)（data-shell・inbox・notifications・projects・board-home）。

## 何が変わったか（task 全体）

- 受信箱 `/inbox`: ADR-0133 の `GET /inbox/items`（判断待ちだけ）を 1 行 1 項目で出し、`POST /inbox/items/{id}/answer` で画面から答える。
  答えた項目は一覧と nav の件数から消える。破壊的な選択は確認、409 `native_action_required` は専用画面へ誘導。
- 通知 `/notifications`（新設）: `GET /notifications` の束、未読・種類・案件の絞り込み、1 件の既読、確認付きの一括既読、頁送り。
  nav の未読数は `/notifications/unread-count`、`inbox_changed`・`notifications_changed` で取り直す。
- `/approvals`・`/reports` は別画面として作り込まず寄せた: 承認は常設ルールと決めた記録だけ（未決は受信箱）、報告は本文を読むだけ（既読は通知）。
- `/projects` は table、`/projects/:id` は途中目標ごとの仕事の木の表と破壊的操作の確認、`/board` は状態ごとの 1 行 1 task の表、
  home は受信箱・通知の 2 つの入口、`/plans/new` は入力欄・失敗表示を DESIGN.md に合わせた。

## verify 葉でしたこと

- `agent-docs/web/feature-parity.md` の R01・R02・R09・R10・R13・R17・R18・R19・R28 を新しい画面に書き換えた（route 行は 42 のまま、
  `/notifications` は R17 の行に書いた。完了列の sha は HEAD の祖先）。
- home の通知入口を history.push の a 要素から `Link to="/notifications"` に戻し、`parity: / home の受信箱・通知の入口と件数`
  （`web/e2e/parity/notifications.spec.ts`）を足した（commit d7e08ee0）。
- quality gate の指摘で nav の「承認」を管理側へ下げ、受信箱の件数と重なる承認待ちの件数 badge（daemon の値が無いと「?」）を外した（commit 1d4f23ec）。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4 件のみ） |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 47 files / 299 tests、node --test pass） |
| `build`・`check:boundaries`・`check:parity`・`check:secrets` | 全て exit 0 |
| `corepack pnpm@12.6.0 -C web mobile-audit` | `31 path(s) x 4 widths ok` |
| `corepack pnpm@12.6.0 -C web e2e`（full、load 1 分値 < 16 を確かめて開始） | 最終コードで 1 回目 exit 0: 218 passed / 8 skipped（17.6 分） |
| `grep -c '^\| R[0-9][0-9] \|' agent-docs/web/feature-parity.md` | 42 |
| `git diff 171c8e02a555 HEAD -- crates gui docs/api web/api/generated \| wc -l` | 0 |
| `screenshots --out <run artifacts>/after-verify` | 32 画面 × 360/390/412/1440 = 124 枚（run の artifacts） |

## quality gate の自己レビュー（受信箱・通知・projects・board・home）

```text
Surface type:     ops workbench（判断待ちの処理と知らせの消化）
Primary task:     受信箱で判断を返す / 通知を読んで既読にする
First decision:   どの判断から答えるか（期限・止めている範囲・推奨が行に並ぶ）
Recovery:         409 は再取得か専用画面への誘導、破壊的な選択と一括既読は確認を経る
Mobile:           360px で横溢れなし（mobile-audit・notifications の 360px 試験）、操作は 44px
```

- 受信箱: 何を決めるか・推奨・期限・止めている範囲・選択肢が 1 行にあり、その場で答えられる。card wall でなく list。合格。
  ただし行の付帯情報（止めている範囲・案件・待ち・関連）の縦の間隔が広く、スマホでは 1 項目が 1 画面を越える（提案へ）。
- 通知: 未読・種類・束の件数・最終時刻が先頭、行き先の link と既読が近い。束ねと既読が読める。合格。
- home: Console の上に件数付きの 2 入口だけで、generic dashboard にしていない。合格。
- projects / board: 表で密度を保ち、状態は token の badge。合格。既定 fixture に案件・task が無く、台帳の screenshot は取得失敗の表示
  （データ入りの姿は projects・board-home 葉の screenshot）。
- 直した点: nav の「承認」が日々の仕事の列に「?」の件数で残り、受信箱と同じ判断を二重に数えていた → 管理側へ下げ、件数を外した。

## 未解決事項

- 偽 daemon の既定 fixture に案件・task・報告が無いので、台帳 screenshot の `/projects`・`/board`・`/reports` は取得失敗の表示（基点から同じ）。
- `use-server-state.ts` は承認待ちの件数を返し続ける（daemon の query は shell の常駐 path を数える e2e が前提にしているので残した）。
- screenshot の台本は goto 直後に撮るため、通知の「すべて既読にする」が読み込み中の無効色で写ることがある。

## 提案

- 受信箱の行の付帯情報を 1〜2 行の meta 行へ詰め、note 欄は note が要る選択を押したときだけ開く（スマホの密度）。
- fake-daemon の既定 fixture に案件・task・報告を 1 件ずつ足し、台帳の screenshot を中身ありにする。
- `GET /projects` に途中目標の集計、`/inbox/items` に案件の題名を足すと一覧の追加取得が減る（API 変更なので別 task）。
