import type { ReportsLive } from "../../api/generated/types";

export function notificationKey(live: ReportsLive | null | undefined): string | null {
  if (!live?.notify_now || live.unread_secretary <= 0) return null;
  return `${live.unread_secretary}:${live.unread_bad_news}`;
}

export function notificationBody(live: ReportsLive): string {
  return live.unread_bad_news > 0
    ? `悪い知らせ ${live.unread_bad_news} 件 / 未読の報告 ${live.unread_secretary} 件`
    : `未読の報告 ${live.unread_secretary} 件`;
}
