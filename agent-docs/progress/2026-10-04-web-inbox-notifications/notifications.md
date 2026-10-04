---
tasks: [01M43690B86J9GHP87CARYM8RS]
unit: notifications
status: done
completed: 2026-10-04
---
# notifications: 通知画面（既読・束ね）と nav の通知入口・未読数、/reports の通知への寄せ

設計は [ADR 2026-10-04-web-inbox-notifications-screens](../../adr/2026-10-04-web-inbox-notifications-screens.md) の D3・D4。
人の決定 notif-route = a（/notifications を新設）。差分の基点は 171c8e02。

## 変えたもの

- `web/routes/notifications.tsx`・`web/features/notifications/`（新規）: `GET /notifications` の束を list で出す
  （未読・種類・束の件数・最終時刻・要約・行き先の link）。絞り込みは URL の `unread`・`kind`・`project`、
  頁は `before`（`next_before`）。1 件の既読（`POST /notifications/{id}/read`）と、絞り込み範囲の一括既読
  （`POST /notifications/read-all`、`ConfirmDialog` で対象・結果・戻せないことを示す）。行き先の link を開くと
  その束を既読にする。ブラウザ通知の許可と「通知を試す」（`POST /notify/test`）は画面の下の「通知の届け方」。
- gateway は `web/server/spa-routes.js` の画面一覧に `/notifications` を 1 行だけ足した（relay・SSE・認証は不変）。
  V3 画面台帳 `web/e2e/support/screens.ts` にも 1 行。
- nav（`components/shell`）: 「通知」の入口を受信箱の次に置き、未読数 badge を `/notifications/unread-count` の
  `unread` から出す（`notifications_changed` で取り直し、15 s poll は補完）。「報告」の入口は管理側へ下げた
  （外すと baseline 7 経路の latency gate が入口を失うため。ADR D4 の選択肢の一つ）。
- `features/reports/notifications-watcher.tsx`・`notification-rule.ts`: ブラウザ通知は unread-count の `events`
  が前に見た値より増えたときに 1 回だけ。許可が無い間は storage に書かない（storage gate）。
- `/reports`: 本文を読むだけの互換の画面にした（h1「報告」・段の絞り込み・展開は保つ）。既読（旧
  `POST /reports/read`）は外し、「通知で報告の知らせを見る」（`/notifications?kind=report`）へ誘導。
  通知の行き先 `/reports?report=<id>` はその報告を開いた状態で出す。
- e2e: `parity/notifications.spec.ts`（新規 5 件）、`shell/notifications-badge.spec.ts`（新規）、
  `parity/reports.spec.ts`（題名は保ち、寄せた後の挙動へ）、常駐 query の path を数える `parity/help`・
  `realtime/refetch-scope` の除外 path に `/api/v1/notifications/unread-count` を足した。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4 件のみ） |
| `corepack pnpm@12.6.0 -C web test` | vitest 46 files / 289 tests passed、node --test pass |
| `check:parity`・`check:boundaries`・`check:secrets`（build 後） | exit 0 |
| e2e 全体（修正前の 1 回目） | 213 passed / 2 failed / 8 skipped。失敗 2 件（latency-gate の nav `/reports`、shell の storage gate）は本 WU 起因で修正済み |
| e2e: parity/latency-gate・shell・reports・notifications、shell/*（修正後） | 27 passed / 1 skipped |
| e2e: parity/notifications・reports、a11y（最終） | 40 passed / 1 skipped |
| `corepack pnpm@12.6.0 -C web mobile-audit` | 31 path × 4 幅 ok |
| screenshots `after-notifications` | 124 枚（台帳 32 行のうち撮れる 31 画面 × 4 幅）（run の artifacts） |
| check 再実行（install・typecheck・lint・test・check:parity）: server/spa-routes.test.mjs の画面数を /notifications 追加に合わせ 32+404 へ | exit 0（vitest 289 passed、node --test 42 passed） |

## 未解決事項

- 偽 daemon の既定 fixture に `/api/v1/reports` が無いので、screenshot の `/reports` は取得失敗の表示になる
  （基点でも同じ path で同じ。parity/reports.spec.ts は自前の fixture で本文まで確かめている）。
- screenshot の台本は `goto` 直後に撮るので、「すべて既読にする」が読み込み中の無効色から遷移する途中に写ることがある。

## 提案

- 承認（/approvals）の nav 入口の扱いは inbox 葉に任せた。両方が管理側へ下がるなら、verify 葉で nav の並びを確かめる。
- 既定 fixture に報告 1 件を足すと /reports の screenshot が本文の状態になる（fixture の葉の範囲）。
