// バッジは画面と同じ key の query に select を掛けて読む（ADR-0081 D5）。バッジ専用の key や cache を作らない。

import type { DaemonSnapshot, HumanInboxView, ReportsLive, UnreadCountView } from "../generated/types";
import { daemonKeys, inboxKeys, notificationKeys } from "./keys";

/** 受信箱の件数バッジ。`GET /inbox/items`（filter 無し）の `counts.total`。受信箱画面の一覧と同じ key。 */
export const inboxItemsBadge = {
  queryKey: inboxKeys.itemList(),
  select: (view: HumanInboxView): number => view.counts.total,
} as const;

/** 通知の未読数バッジ。`GET /notifications/unread-count` の未読の束数（束内の出来事の和ではない）。 */
export const notificationsUnreadBadge = {
  queryKey: notificationKeys.unreadCount(),
  select: (view: UnreadCountView): number => view.unread,
} as const;

/** 報告・認可のバッジ。REST の daemon query（daemon/rest）と同じ key。SSE の daemon/stream は根拠にしない。 */
export const daemonBadges = {
  queryKey: daemonKeys.rest(),
  select: (daemon: DaemonSnapshot): { approvalsPending: number | null; reports: ReportsLive | null } => ({
    approvalsPending: daemon.approvals_pending ?? null,
    reports: daemon.reports ?? null,
  }),
} as const;

export type BadgeView = { text: string; label: string } | null;

/** 値が不明（loading / error / undefined / null）のとき 0 にしない。"?" と読み上げ用の label を返す。既知の 0 は出さない。 */
export function badgeView(value: number | null | undefined, name: string): BadgeView {
  if (value === null || value === undefined || !Number.isFinite(value))
    return { text: "?", label: `${name}の件数は不明` };
  if (value <= 0) return null;
  return { text: value > 99 ? "99+" : String(value), label: `${name} ${value} 件` };
}
