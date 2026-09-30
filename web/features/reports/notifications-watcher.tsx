import { useEffect } from "react";
import { apiMutate } from "../../api/client";
import type { ReportsLive } from "../../api/generated/types";
import { notificationBody, notificationKey } from "./notification-rule";

// origin ごとの保存領域には件数の鍵だけを残す。報告本文は保存しない。
const storageKey = "celeris-web-reports-notification-key";
let fallbackKey: string | null = null;

function getKey(): string | null {
  try {
    return localStorage.getItem(storageKey);
  } catch {
    return fallbackKey;
  }
}
function setKey(key: string): void {
  fallbackKey = key;
  try {
    localStorage.setItem(storageKey, key);
  } catch {
    /* storage が使えない場合はタブ内だけで重複排除 */
  }
}

export function NotificationsWatcher({ reportsLive }: { reportsLive: ReportsLive | null }) {
  const key = notificationKey(reportsLive);
  useEffect(() => {
    if (!reportsLive || !key || typeof Notification === "undefined" || Notification.permission !== "granted") return;
    const notify = async () => {
      if (getKey() === key || Notification.permission !== "granted") return;
      setKey(key);
      new Notification("celeris: 報告", { body: notificationBody(reportsLive) });
      try {
        await apiMutate("POST", "/api/reports/notified");
      } catch {
        /* 通知は既に表示済み。再表示しない */
      }
    };
    // Web Locks で同一 origin のタブ間の確認・書き込みを直列化する。
    if (navigator.locks) void navigator.locks.request(storageKey, notify);
    else void notify();
  }, [key, reportsLive]);
  return null;
}
