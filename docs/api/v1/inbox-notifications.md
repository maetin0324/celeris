# 受信箱と通知の API（ADR-0133 D5）

---
tasks: [01M3YFCJKMNWQ13HRS52M5BSWW]
---

base path は `/api/v1`。JSON の正本は [`api-v1.schema.json`](api-v1.schema.json)（`HumanInboxView`・`InboxAnswerResult`・`NotificationsView` など）。

`GET /inbox/items?project=&kind=` は判断待ちだけを `HumanInboxView`（`items`、`counts`、`suppressed`）で返す。`GET /inbox/items/{id}` は 1 件を返し、回答済み・失効済みなら 404。`POST /inbox/items/{id}/answer` は管理系で、`{"option":"...","note":"...","payload":{}}` を受ける。`option` はその項目の `options[].key` に限る。成功時は `InboxAnswerResult`（`removed`、`item_id`、委ね先の `result`）。browser の回答は `payload` に既存 browser API の attestation と version を渡し、資格情報そのものは送らない。KB の取り込み待ち（`_inbox/` の候補）は受信箱に出さない（`InboxKind` に `knowledge_review` は無い。人の決定 2026-10-08、ADR-0133 付記）。候補の受け入れ・却下は `POST /knowledge/inbox/{id}/accept|reject` で行い、`GET /inbox/items/knowledge_review` とその answer は 404 `inbox-item-gone`。認可、決定、task gate の効き目は既存の領域 API に委ねる。`unroutable` の `reassign` は `payload` に既存の `PATCH /tasks/{id}` の編集本文を渡す。クラスタの再接続など、GUI の専用操作を要する項目は 409 `native_action_required` を返す。

`GET /notifications?unread=&kind=&project=&limit=&before=` は通知の束を `NotificationsView`（`items`、`unread`、`next_before`）で返す。`before` は RFC 3339 の `last_at`、`limit` は 1〜500（既定 50）。`GET /notifications/unread-count` は未読の束数 `unread`、束内の出来事の和 `events`、`by_kind` を返す。`POST /notifications/{id}/read` は管理系の冪等な既読化で `id` と `read_at` を返す。`POST /notifications/read-all` は管理系で `{"before"?: RFC3339, "kind"?: NoticeKind, "project"?: string}` を受け、`{"marked": n}` を返す。

旧 `GET /inbox`、`POST /reports/read`、`POST /reports/notified` は互換の形を保ち、`Deprecation: true` と後継を示す `Link` を付ける。`/approvals`、`/decisions`、`/browser/waits`、`/knowledge/inbox`、`/reports` の読み取り、`/notify` は領域の API として残る。SSE `/stream` は task event の後に `inbox_changed`、通知台帳が変わったときに `notifications_changed` の軽い合図を出す。廃止時期は ADR-0133 D5 の人の決定に従う。
