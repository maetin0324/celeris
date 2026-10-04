import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { daemonBadges, inboxItemsBadge } from "../../api/queries/badges";
import { inboxItemsQuery, unreadCountQuery } from "../../api/queries/inbox-notifications";
import {
  DAEMON_INTERVAL_MS,
  daemonRestQuery,
  HEALTH_INTERVAL_MS,
  healthQuery,
  INBOX_INTERVAL_MS,
  pollInterval,
} from "../../api/queries/server-state";

function useDocumentVisible(): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}

/**
 * shell の server state。shell の mount は結果で変えない。
 * `authenticated` は root の gate（gateway の session）を通った shell では true。
 */
export function useShellServerState(authenticated: boolean) {
  const queryClient = useQueryClient();
  const visible = useDocumentVisible();
  const active = authenticated && visible;

  const health = useQuery({
    ...healthQuery,
    enabled: authenticated,
    refetchInterval: pollInterval(HEALTH_INTERVAL_MS, active),
    refetchIntervalInBackground: false,
  });
  const daemon = useQuery({
    ...daemonRestQuery,
    ...daemonBadges,
    enabled: authenticated,
    refetchInterval: pollInterval(DAEMON_INTERVAL_MS, active),
    refetchIntervalInBackground: false,
  });
  const inbox = useQuery({
    ...inboxItemsQuery(),
    ...inboxItemsBadge,
    enabled: authenticated,
    refetchInterval: pollInterval(INBOX_INTERVAL_MS, active),
    refetchIntervalInBackground: false,
  });

  // 通知の未読数（nav の badge とブラウザ通知の根拠）。notifications_changed で取り直し、15 s の poll は補完。
  const notifications = useQuery({
    ...unreadCountQuery,
    enabled: authenticated,
    refetchInterval: pollInterval(INBOX_INTERVAL_MS, active),
    refetchIntervalInBackground: false,
  });

  // 断 → 復旧の遷移で、表示中（active）の query だけ取り直す。
  const wasDown = useRef(false);
  const down = health.isError;
  useEffect(() => {
    if (down) {
      wasDown.current = true;
    } else if (health.isSuccess && wasDown.current) {
      wasDown.current = false;
      void queryClient.invalidateQueries({ refetchType: "active" });
    }
  }, [down, health.isSuccess, queryClient]);

  return {
    down,
    inboxBadge: inbox.data ?? null,
    approvalsBadge: daemon.data?.approvalsPending ?? null,
    notificationsBadge: notifications.data?.unread ?? null,
    notificationsUnread: notifications.data ?? null,
  };
}
