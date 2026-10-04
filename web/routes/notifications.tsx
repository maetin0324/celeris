import { createFileRoute } from "@tanstack/react-router";
import type { NoticeKind } from "../api/generated/types";
import { isNoticeKind } from "../features/notifications/notice-view";
import { NotificationsScreen } from "../features/notifications/notifications-screen";
import { optionalBoolean, optionalString } from "../lib/search";

export type NotificationsSearch = { unread?: boolean; kind?: NoticeKind; project?: string; before?: string };

export const Route = createFileRoute("/notifications")({
  validateSearch: (search: Record<string, unknown>): NotificationsSearch => {
    const kind = optionalString(search.kind);
    return {
      unread: optionalBoolean(search.unread) === true ? true : undefined,
      kind: isNoticeKind(kind) ? kind : undefined,
      project: optionalString(search.project),
      before: optionalString(search.before),
    };
  },
  component: NotificationsScreen,
});
