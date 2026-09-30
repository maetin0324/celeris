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
  select: (daemon: DaemonSnapshot): { approvalsPending: number; reports: ReportsLive | null } => ({
    approvalsPending: daemon.approvals_pending ?? 0,
    reports: daemon.reports ?? null,
  }),
} as const;
