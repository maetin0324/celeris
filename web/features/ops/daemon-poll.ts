// /daemon の補完取得（P4-12）。SSE の `daemon` tick では取り直さず、表示中だけ上限付きで polling する。
// 上限: 周期 10 s、自動の取得は 30 回（5 分）まで。それ以降は「再取得」ボタンか操作の後の取り直しで更新する。
export const DAEMON_SCREEN_INTERVAL_MS = 10_000;
export const DAEMON_SCREEN_MAX_POLLS = 30;

export function daemonPollInterval(fetchCount: number, visible = true): number | false {
  if (!visible) return false;
  return fetchCount < DAEMON_SCREEN_MAX_POLLS ? DAEMON_SCREEN_INTERVAL_MS : false;
}
