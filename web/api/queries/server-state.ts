// shell が持つ server state の query（P2-03、ADR-0081 D5）。key は keys.ts のものだけを使う。
// 補完取得の周期: daemon/rest 5 s、inbox 15 s、health 5 s。画面が見えていて認証済みの間だけ走らせる。

import { apiGet } from "../client";
import type { DaemonSnapshot, Health, Inbox } from "../generated/types";
import { daemonKeys, healthKeys, inboxKeys } from "./keys";

export const HEALTH_INTERVAL_MS = 5_000;
export const DAEMON_INTERVAL_MS = 5_000;
export const INBOX_INTERVAL_MS = 15_000;

export const healthQuery = {
  queryKey: healthKeys.all,
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Health>("/api/health", signal),
  retry: false,
} as const;

export const daemonRestQuery = {
  queryKey: daemonKeys.rest(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<DaemonSnapshot>("/api/daemon", signal),
} as const;

/** 旧 `GET /inbox`（ADR-0133 D5 で Deprecation 付きの互換）。nav の件数は inbox-notifications.ts の `/inbox/items`。 */
export const inboxQuery = {
  queryKey: inboxKeys.list(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Inbox>("/api/inbox", signal),
} as const;

/** 見えていて認証済みのときだけ周期を返す。それ以外は false（止める）。 */
export function pollInterval(ms: number, active: boolean): number | false {
  return active ? ms : false;
}
