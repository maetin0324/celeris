import { useEffect } from "react";
import type { UnreadCountView } from "../../api/generated/types";
import { notificationBody, shouldNotify } from "./notification-rule";

// origin ごとの保存領域には出来事の数だけを残す。通知の本文は保存しない。
const storageKey = "celeris-web-notifications-seen-events";
let fallbackSeen: number | null = null;

function getSeen(): number | null {
  try {
    const value = localStorage.getItem(storageKey);
    return value === null ? fallbackSeen : Number(value);
  } catch {
    return fallbackSeen;
  }
}
function setSeen(value: number): void {
  fallbackSeen = value;
  try {
    localStorage.setItem(storageKey, String(value));
  } catch {
    /* storage が使えない場合はタブ内だけで重複排除 */
  }
}

/** 通知の未読（unread-count）の出来事が増えたらブラウザ通知を 1 回出す。タブ間は Web Locks で直列化する。 */
export function NotificationsWatcher({ unread }: { unread: UnreadCountView | null }) {
  const events = unread?.events ?? null;
  useEffect(() => {
    if (!unread || events === null) return;
    const check = async () => {
      const seen = getSeen();
      if (!shouldNotify(unread, seen)) {
        if (seen === null || events < seen) setSeen(events);
        return;
      }
      setSeen(events);
      if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
      new Notification("celeris: 通知", { body: notificationBody(unread) });
    };
    if (navigator.locks) void navigator.locks.request(storageKey, check);
    else void check();
  }, [events, unread]);
  return null;
}
