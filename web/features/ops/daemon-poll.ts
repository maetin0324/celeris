// /daemon の補完取得（P4-12）。SSE の `daemon` tick では取り直さず、表示中だけ上限付きで polling する。
// 上限: 周期 10 s、自動の取得は 30 回（5 分）まで。それ以降は「再取得」ボタンか操作の後の取り直しで更新する。
export const DAEMON_SCREEN_INTERVAL_MS = 10_000;
export const DAEMON_SCREEN_MAX_POLLS = 30;
// 最後の tick がこれより古ければ、dispatcher が止まっている疑いとして文字で示す。
export const DAEMON_STALE_TICK_MS = 60_000;

export function daemonPollInterval(fetchCount: number, visible = true): number | false {
  if (!visible) return false;
  return fetchCount < DAEMON_SCREEN_MAX_POLLS ? DAEMON_SCREEN_INTERVAL_MS : false;
}

/** 自動 polling が上限に達したか。達したら画面に「自動更新を止めた」と出す。 */
export function daemonPollExhausted(fetchCount: number): boolean {
  return fetchCount >= DAEMON_SCREEN_MAX_POLLS;
}

export type DaemonLiveness = "running" | "stale" | "absent";

/** 稼働状態: snapshot が無ければ absent、最後の tick が now より DAEMON_STALE_TICK_MS 以上古ければ stale。 */
export function daemonLiveness(now: string, lastTickAt: string | null | undefined): DaemonLiveness {
  if (!lastTickAt) return "absent";
  const gap = Date.parse(now) - Date.parse(lastTickAt);
  if (Number.isNaN(gap)) return "running";
  return gap >= DAEMON_STALE_TICK_MS ? "stale" : "running";
}
