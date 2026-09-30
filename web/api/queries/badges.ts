// バッジは画面と同じ key の query に select を掛けて読む（ADR-0081 D5）。バッジ専用の key や cache を作らない。

import type { DaemonSnapshot, Inbox, InboxCounts, ReportsLive } from "../generated/types";
import { daemonKeys, inboxKeys } from "./keys";

/** inbox の件数バッジ。画面の inbox 一覧（filter 無し）と同じ key。 */
export const inboxCountsBadge = {
  queryKey: inboxKeys.list(),
  select: (inbox: Inbox): InboxCounts => inbox.counts,
} as const;

/** 報告・認可のバッジ。REST の daemon query（daemon/rest）と同じ key。SSE の daemon/stream は根拠にしない。 */
export const daemonBadges = {
  queryKey: daemonKeys.rest(),
  select: (daemon: DaemonSnapshot): { approvalsPending: number | null; reports: ReportsLive | null } => ({
    approvalsPending: daemon.approvals_pending ?? null,
    reports: daemon.reports ?? null,
  }),
} as const;

/** 受信箱ナビのバッジ用の合計。 */
export function inboxTotal(counts: InboxCounts): number {
  return (
    counts.approvals + counts.attention + counts.browser_waits + counts.decisions + counts.drafts + counts.questions
  );
}

export type BadgeView = { text: string; label: string } | null;

/** 値が不明（loading / error / undefined / null）のとき 0 にしない。"?" と読み上げ用の label を返す。既知の 0 は出さない。 */
export function badgeView(value: number | null | undefined, name: string): BadgeView {
  if (value === null || value === undefined || !Number.isFinite(value))
    return { text: "?", label: `${name}の件数は不明` };
  if (value <= 0) return null;
  return { text: value > 99 ? "99+" : String(value), label: `${name} ${value} 件` };
}
