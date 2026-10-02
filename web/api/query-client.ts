// session に 1 つの QueryClient（ADR-0081 D2・D5、P2-01）。
// - GET（query）だけ上限付きで再試行する。401 / 403 / 検証エラーなど、繰り返しても結果が変わらないものは再試行しない。
// - 変更系（mutation）は再試行も、offline からの復帰での自動再送もしない。
// - logout・session 失効・切替で query と mutation の cache を捨て、進行中の fetch を止める。
// - cache を永続化しない（H4）。persist 系の package は入れない。

import { QueryClient } from "@tanstack/react-query";
import { registerProtectedCache } from "../lib/session";
import { type ApiErrorKind, abortSessionRequests, isApiError } from "./client";
import { applyStaleTimeDefaults } from "./queries/stale-time";

/** 初回を含めない再試行の回数の上限。 */
export const MAX_QUERY_RETRIES = 2;

const RETRYABLE: ReadonlySet<ApiErrorKind> = new Set(["timeout", "network", "server", "rate_limited"]);

export function shouldRetryQuery(failureCount: number, error: unknown): boolean {
  if (failureCount >= MAX_QUERY_RETRIES) return false;
  return isApiError(error) && error.method === "GET" && RETRYABLE.has(error.kind);
}

export function queryRetryDelay(attempt: number): number {
  return Math.min(1_000 * 2 ** attempt, 8_000);
}

export function createQueryClient(): QueryClient {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: shouldRetryQuery,
        retryDelay: queryRetryDelay,
      },
      mutations: {
        retry: false,
        // 既定の "online" は offline 中の mutation を止めて復帰時に送り直す。変更系は自動で再送しない。
        networkMode: "always",
      },
    },
  });
  applyStaleTimeDefaults(client);
  return client;
}

let current: QueryClient | undefined;

/** 今の session の QueryClient。session の終了後は新しいものを作る。 */
export function getSessionQueryClient(): QueryClient {
  current ??= createQueryClient();
  return current;
}

/** session の終了（logout・失効・切替）。進行中の fetch を止め、query と mutation の cache を捨てる。 */
export function endQuerySession(): void {
  abortSessionRequests();
  const client = current;
  current = undefined;
  if (!client) return;
  void client.cancelQueries();
  client.clear(); // query と mutation の cache の両方
}

/** lib/session の保護 cache に登録する（401 と logout で clearProtectedCaches が呼ぶ）。戻り値で外す。 */
export function bindQueryClientToSession(): () => void {
  return registerProtectedCache(endQuerySession);
}
