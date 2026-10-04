# web の受信箱・通知の画面と data 層（2026-10-04）

状態: 採用（task 01M43690B86J9GHP87CARYM8RS）。D1〜D3・D6 は実装済み（WU data-shell）。D4・D5 は同じ task の画面の葉（inbox・notifications・
board-home・projects）が実装する。

関連: ADR-0133（受信箱と通知の 2 系統、`docs/api/v1/inbox-notifications.md`）、ADR-0081（web/ SPA、D5 の
Query key・D6 の SSE invalidate）、web-0004（design system）、`docs/frontend/DESIGN.md`・`FRONTEND_CONTRACT.md`。

## 背景

ADR-0133 で daemon に「受信箱 = 人の判断が要るものだけ（答えると消える）」と「通知 = 判断の要らない知らせ
（既読・束ね）」の 2 系統 API と SSE の合図（`inbox_changed`・`notifications_changed`）が入った。web/ は
まだ旧 `GET /inbox`（`approvals`・`questions`・`drafts`・`attention`・`browser_waits`・`decisions` の 6 種の束）
を読み、nav の受信箱の件数もその 6 種の和だった。判断の要らない attention も数えるので、件数が「人が今
決めること」の数にならない。報告（`/reports`）と承認（`/approvals`）も別の画面と nav の入口を持っている。

## 決定

### D1. data 層は `web/api/queries` への最小の追加にする

- key は `keys.ts` に足す。受信箱は `['inbox','items',filters]`・`['inbox','item',id]`、通知は
  `['notifications','list',filters]`・`['notifications','unread-count']`。受信箱の新しい key は旧
  `GET /inbox` の `['inbox',filters]` と同じ `['inbox']` の下に置く。既存の task.event の `L`（受信箱を含む一覧）
  と inbox_changed の invalidate が、どちらの画面にもそのまま効く。staleTime は `['notifications']` を
  `['inbox']` と同じ 5 s にする。
- query と mutation は `inbox-notifications.ts` にまとめる（`inboxItemsQuery`・`inboxItemQuery`・
  `answerInboxItem`・`notificationsQuery`・`unreadCountQuery`・`markNotificationRead`・
  `markAllNotificationsRead`）。URL の query 文字列は key と同じ `normalizeFilters` の結果から作り、key と
  要求がずれないようにする。型は生成型（`HumanInboxView`・`InboxItem`・`InboxAnswerBody`・`InboxAnswerResult`・
  `NotificationsView`・`UnreadCountView`・`NoticeReadResult`・`NoticeReadAllResult`・`ReadAllBody`）をそのまま
  使い、web 側で写しの型を作らない。
- 件数 badge は既存の規則（画面と同じ key に `select`、専用の cache を作らない）に従う。受信箱は
  `counts.total`、通知は `unread`（未読の**束**の数。束内の出来事の和 `events` ではない。nav の数字は
  「開いて読むものの数」にする）。
- 理由: API 型・gateway の relay と events・crates/ を変えずに済む。gateway は `/api/*` を `/api/v1/*` へ
  中継し、SSE は event 名を問わず流すので、新しい path と合図はそのまま通る（e2e で確かめた）。

### D2. SSE の合図は中身を読まず、対応する key 全体を束ねて取り直す

- `FRAME_TYPES` に `inbox_changed`・`notifications_changed` を足す。data は `{}` で、将来の欄が増えても
  捨てないよう parse で中身を見ない。
- `SIGNAL_INVALIDATION`（`invalidation-map.ts`）で `inbox_changed → ['inbox']`、
  `notifications_changed → ['notifications']` とし、invalidator の 250 ms の束ねと in-flight 1 本の規則に乗せる。
  互いの key・task・daemon の key には触れない（`signals.test.ts`）。

### D3. nav の受信箱の件数は `GET /inbox/items` の `counts.total` から出す

- shell の 15 s の補完 poll も `/inbox/items` に移す。旧 `GET /inbox` は受信箱画面が inbox 葉で移るまでの
  互換で、shell からは呼ばない。
- 通知の入口と未読数 badge（`notificationsUnreadBadge`）は notifications 葉が nav に足す。

### D4. 承認・報告は別画面として作り込まず、受信箱・通知へ寄せる

- `/approvals`: 認可は受信箱の `authorization` 項目として答える。route は互換の最小限で残し（parity の
  h1「承認」と URL を保つ）、受信箱の `kind=authorization` への誘導と件数だけを出す。
- `/reports`: 報告は通知の `report` 束として既読にする。route は互換の最小限で残し（h1「報告」を保つ）、
  通知の `kind=report` への誘導を出す。
- nav は「受信箱」「通知」を日々の仕事の先頭に置き、承認・報告の入口は通知の入口と同じ葉で外すか管理側へ
  下げる（parity の nav 検査を確かめた上で notifications 葉が決める）。

### D5. board は card wall にせず list/table にする

- 状態ごとの列のカードではなく、web-0004 の `Table` primitive で 1 行 1 task の表にし、状態は列と
  `StatusBadge` で読む。状態での絞り込みと件数は表の上に置く。スマホ幅でも横に溢れない行の形にする。
- home と案件の画面も、generic dashboard（数字のタイルの並び）にせず、受信箱の判断待ちと進行中の task の
  list を主にする。

### D6. fake daemon の受信箱・通知は状態を持つ

- `web/e2e/support/fake-daemon.mjs` は 5 種の受信箱の項目（`decision`・`plan_gate`・`failed`・
  `authorization`・`knowledge_review`。answer で消え、`needs_note` の選択肢は note が無いと 422）と 4 束の通知
  （未読 3・既読 1。`read`・`read-all` の `kind`/`project`/`before` 条件、`limit`/`before` の頁送り）を持つ。
  変わったら SSE の合図を送る。試験は `setInboxItems`・`setNotices` で差し替えられ、`fixtures` に同じ path を
  渡せばそちらが勝つ。応答は schema で検証する（`fake-daemon.test.ts`）。

## 結果

- 受信箱・通知の画面の葉は、この data 層と fixture の上で画面だけを作る。
- shell の poll 先が変わったので、shell の要求を数える e2e（`refetch-scope`・`help`・`shell`・`latency-gate`）
  の path を `/api/v1/inbox/items` に合わせた。
