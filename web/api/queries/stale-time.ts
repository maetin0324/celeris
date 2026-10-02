// staleTime の初期値（ADR-0081 D5 の表、ミリ秒）。「再利用してよい期間」であり定期取得の周期ではない。
// QueryClient には prefix ごとの既定値として入れる（一般 → 特定の順。後に入れたものが勝つ）。

import type { QueryClient, QueryKey } from "@tanstack/react-query";

/** 終了済みの run のログ（D5「終了済みログは 60,000」）。使う側が query ごとに指定する。 */
export const FINISHED_LOG_STALE_TIME = 60_000;

export const STALE_TIME_DEFAULTS: readonly (readonly [QueryKey, number])[] = [
  [["tasks"], 5_000],
  [["tasks", "timeline"], 1_000],
  [["tasks", "runs"], 1_000],
  [["tasks", "run"], 1_000],
  [["projects"], 10_000],
  [["projects", "docs"], 60_000],
  [["inbox"], 5_000],
  [["board"], 5_000],
  [["reports"], 5_000],
  [["approvals"], 5_000],
  [["daemon", "rest"], 5_000],
  [["health"], 5_000],
  [["daemon", "stream"], 0],
  [["providers"], 10_000],
  [["accounts"], 10_000],
  [["clusters"], 10_000],
  [["releases"], 10_000],
  [["metrics"], 10_000],
  [["org"], 60_000],
  [["knowledge"], 60_000],
  [["skills"], 60_000],
  [["config"], 60_000],
  [["mcp"], 60_000],
  [["console"], 0],
];

function isPrefix(prefix: QueryKey, key: QueryKey): boolean {
  return prefix.length <= key.length && prefix.every((part, i) => part === key[i]);
}

/** 表から key の staleTime を引く。最も長い prefix が勝つ。表に無い key は 0。 */
export function staleTimeFor(key: QueryKey): number {
  let best: readonly [QueryKey, number] | undefined;
  for (const entry of STALE_TIME_DEFAULTS) {
    if (isPrefix(entry[0], key) && (!best || entry[0].length > best[0].length)) best = entry;
  }
  return best ? best[1] : 0;
}

export function applyStaleTimeDefaults(client: QueryClient): void {
  const ordered = [...STALE_TIME_DEFAULTS].sort((a, b) => a[0].length - b[0].length);
  for (const [prefix, staleTime] of ordered) client.setQueryDefaults(prefix, { staleTime });
}
