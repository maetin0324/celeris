---
tasks: [01M43690B86J9GHP87CARYM8RS]
unit: data-shell
status: done
completed: 2026-10-04
---
# data-shell: 受信箱・通知の data 層・SSE 合図・fixture と nav の受信箱件数

設計は [ADR 2026-10-04-web-inbox-notifications-screens](../../adr/2026-10-04-web-inbox-notifications-screens.md)（D1〜D3・D6 を実装）。差分の基点は 171c8e02。

## 変えたもの

- `web/api/queries/keys.ts`・`stale-time.ts`: `inboxKeys.items/itemList/item`、`notificationKeys`（list・unread-count）、staleTime 5 s。
- `web/api/queries/inbox-notifications.ts`（新規）: `GET /inbox/items`・`/inbox/items/{id}`・`POST answer`、`GET /notifications`・`/unread-count`・`POST /{id}/read`・`/read-all`。生成型をそのまま使う。
- `web/api/queries/badges.ts`: `inboxItemsBadge`（counts.total）・`notificationsUnreadBadge`（unread）。旧 `inboxCountsBadge`・`inboxTotal` は使う所が無くなったので外した。
- `web/api/realtime/{frames,invalidation-map,realtime}.ts`: `inbox_changed → ['inbox']`、`notifications_changed → ['notifications']`。
- `web/components/shell/use-server-state.ts`: nav の受信箱件数を `/inbox/items` の counts.total から出す。
- `web/e2e/support/fake-daemon.mjs`（+ `.d.mts`）: 受信箱 5 種（decision・plan_gate・failed・authorization・knowledge_review）と通知 4 束の状態、answer・read・read-all、SSE 合図、`setInboxItems`・`setNotices`。
- e2e: `web/e2e/shell/inbox-badge.spec.ts`（新規）。shell の poll 先の変更に合わせて `refetch-scope`・`help`・`shell`・`latency-gate` の数える path を `/api/v1/inbox/items` にした。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4 件のみ） |
| `corepack pnpm@12.6.0 -C web exec vitest run` | 44 files / 284 tests passed |
| `corepack pnpm@12.6.0 -C web test`（vitest + node --test） | exit 0、node 42 pass |
| `check:boundaries`・`check:parity` | exit 0 |
| e2e: parity/shell・help・inbox・latency-gate、realtime/refetch-scope、shell/*、a11y | 82 passed / 2 skipped |
| e2e: shell/inbox-badge.spec.ts | 1 passed（5 件 → inbox_changed で 4 件 → 0 件で badge 消える） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-adr-numbers.sh`・`check-doc-links.sh` | ok |
| screenshots `after-data-shell` | 31 画面 × 4 幅 = 120 枚（run の artifacts） |

## 未解決事項

- 受信箱画面（/inbox）はまだ旧 `GET /inbox` を読む（inbox 葉で移す）。nav の件数（判断待ちだけ）と画面の件数が一時的に食い違い得る。
- 通知の nav 入口と未読数 badge の配線は notifications 葉。

## 提案

- 承認・報告の nav 入口を外すか管理側へ下げるかは、parity の nav 検査を確かめて notifications 葉で決める（ADR D4）。
