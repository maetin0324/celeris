import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import {
  applyStreamBlock,
  applyStreamHello,
  buildInstructBody,
  type ConsoleCache,
  consoleQueryKey,
  emptyCache,
  fetchConsole,
  sendInstruct,
} from "./cache";
import { openConsoleStream } from "./stream";

// Console の hook（P3-01）。REST の初回取得と stream の差分は同じ cache key に入る。
// `/events` の再取得（api/realtime）とは別の接続。console の key は invalidation の対象にしない。

export function useConsole(scope: string) {
  const client = useQueryClient();
  const key = consoleQueryKey(scope);
  // The Console reads the scope from its start: there is no "new conversation" cut-off
  // (ADR 2026-10-22-cos-fixed-threads D2').
  const since = null;
  const query = useQuery<ConsoleCache>({
    queryKey: key,
    queryFn: ({ signal }) => fetchConsole(client, key, scope, since, signal),
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const cursorRef = useRef<string | null>(null);
  cursorRef.current = query.data?.cursor ?? since;
  const keyHash = key.join("\u0000");
  // stream は初回取得が決着してから張る。cursor が REST の続きになり、daemon が遅いときに
  // 取得中の fetch と SSE 2 本で同一 host の接続上限（6）を埋めて次の document 遷移を待たせない。
  // 初回取得に失敗したときは張らない。stream の hello が空の cache を書くと error が消えて
  // 「まだ会話がありません」と誤って出るため。再試行で取れてから張る。
  const settled = query.status === "success";
  // biome-ignore lint/correctness/useExhaustiveDependencies: scope が変わったときだけ張り直す
  useEffect(() => {
    if (!settled) return;
    const stream = openConsoleStream({
      scope,
      since: cursorRef.current,
      onHello: (hello) => applyStreamHello(client, key, hello.cursor),
      onBlock: (block) => applyStreamBlock(client, key, block),
    });
    return () => stream.close();
  }, [client, scope, keyHash, settled]);
  return { ...query, blocks: query.data?.blocks ?? emptyCache.blocks, cursor: query.data?.cursor ?? null };
}

/** 送信。202 を受けるまで pending で、その間は二重に出さない。 */
export function useSendInstruct(scope: string) {
  const inflight = useRef(false);
  const mutation = useMutation({ mutationFn: sendInstruct });
  return {
    pending: mutation.isPending,
    error: mutation.error,
    async send(text: string): Promise<boolean> {
      if (inflight.current) return false;
      inflight.current = true;
      try {
        await mutation.mutateAsync(buildInstructBody(text, scope));
        return true;
      } catch {
        return false;
      } finally {
        inflight.current = false;
      }
    },
  };
}
