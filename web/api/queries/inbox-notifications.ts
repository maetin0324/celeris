// 受信箱（人の判断）と通知（知らせ）の query と mutation（ADR-0133 D5、docs/api/v1/inbox-notifications.md）。
// key は keys.ts のものだけを使う。URL の query 文字列は key と同じ正規化済みの filter から作る。

import { apiGet, apiMutate } from "../client";
import type {
  HumanInboxView,
  InboxAnswerBody,
  InboxAnswerResult,
  InboxItem,
  InboxKind,
  NoticeKind,
  NoticeReadAllResult,
  NoticeReadResult,
  NotificationsView,
  ReadAllBody,
  UnreadCountView,
} from "../generated/types";
import { inboxKeys, notificationKeys } from "./keys";
import { type NormalizedFilters, normalizeFilters } from "./normalize";

export type InboxItemsFilters = { project?: string; kind?: InboxKind };
export type NotificationsFilters = {
  unread?: boolean;
  kind?: NoticeKind;
  project?: string;
  limit?: number;
  before?: string;
};

/** 正規化済みの filter を URL の query にする（key と同じ並び・同じ値）。 */
export function searchOf(filters: NormalizedFilters): string {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(filters)) {
    if (Array.isArray(value)) for (const item of value) params.append(name, String(item));
    else params.set(name, String(value));
  }
  const text = params.toString();
  return text === "" ? "" : `?${text}`;
}

const segment = (id: string) => encodeURIComponent(id);

/** `GET /inbox/items?project=&kind=`。判断待ちだけ（`counts.total` が nav の件数）。 */
export function inboxItemsQuery(filters?: InboxItemsFilters) {
  const normalized = normalizeFilters(filters);
  return {
    queryKey: inboxKeys.itemList(filters),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<HumanInboxView>(`/api/inbox/items${searchOf(normalized)}`, signal),
  } as const;
}

/** `GET /inbox/items/{id}`。回答済み・失効済みは 404（ApiError の not_found）。 */
export function inboxItemQuery(itemId: string) {
  return {
    queryKey: inboxKeys.item(itemId),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<InboxItem>(`/api/inbox/items/${segment(itemId)}`, signal),
  } as const;
}

/** `POST /inbox/items/{id}/answer`。`option` は項目の `options[].key` に限る。 */
export function answerInboxItem(itemId: string, body: InboxAnswerBody, signal?: AbortSignal) {
  return apiMutate<InboxAnswerResult>("POST", `/api/inbox/items/${segment(itemId)}/answer`, body, signal);
}

/** 回答の後に stale にする key。受信箱は旧 `GET /inbox` も含めて全体、通知は答えでは変わらない。 */
export const inboxAnswerInvalidates = [inboxKeys.all] as const;

/** `GET /notifications?unread=&kind=&project=&limit=&before=`。束の一覧。 */
export function notificationsQuery(filters?: NotificationsFilters) {
  const normalized = normalizeFilters(filters);
  return {
    queryKey: notificationKeys.list(filters),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<NotificationsView>(`/api/notifications${searchOf(normalized)}`, signal),
  } as const;
}

/** `GET /notifications/unread-count`。nav の未読数の根拠。 */
export const unreadCountQuery = {
  queryKey: notificationKeys.unreadCount(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<UnreadCountView>("/api/notifications/unread-count", signal),
} as const;

/** `POST /notifications/{id}/read`（冪等）。 */
export function markNotificationRead(noticeId: string, signal?: AbortSignal) {
  return apiMutate<NoticeReadResult>("POST", `/api/notifications/${segment(noticeId)}/read`, undefined, signal);
}

/** `POST /notifications/read-all`。条件を付けなければ全部。 */
export function markAllNotificationsRead(body: ReadAllBody = {}, signal?: AbortSignal) {
  return apiMutate<NoticeReadAllResult>("POST", "/api/notifications/read-all", body, signal);
}

/** 既読化の後に stale にする key（一覧と未読数）。 */
export const notificationReadInvalidates = [notificationKeys.all] as const;
