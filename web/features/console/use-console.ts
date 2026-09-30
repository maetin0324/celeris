import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useSyncExternalStore } from "react";
import {
  applyStreamBlock,
  applyStreamHello,
  buildInstructBody,
  type ConsoleCache,
  consoleQueryKey,
  emptyCache,
  fetchConsole,
  sendInstruct,
  startNewConversation,
} from "./cache";
import { conversationStore } from "./store";
import { openConsoleStream } from "./stream";

// Console の hook（P3-01）。REST の初回取得と stream の差分は同じ cache key に入る。
// `/events` の再取得（api/realtime）とは別の接続。console の key は invalidation の対象にしない。

export function useConversation(scope: string) {
  return useSyncExternalStore(
    conversationStore.subscribe,
    () => conversationStore.get(scope),
    () => conversationStore.get(scope),
  );
}

export function useConsole(scope: string) {
  const client = useQueryClient();
  const conversation = useConversation(scope);
  const key = consoleQueryKey(scope, conversation.id);
  const since = conversation.since;
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
  const settled = query.status !== "pending";
  // biome-ignore lint/correctness/useExhaustiveDependencies: scope と会話が変わったときだけ張り直す
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

/** 新しい会話。成功したら、いまの cursor 以降だけを見る新しい会話 id に切り替える。 */
export function useNewConversation(scope: string, cursor: string | null) {
  const client = useQueryClient();
  const mutation = useMutation({
    mutationFn: startNewConversation,
    onSuccess: () => {
      conversationStore.next(scope, cursor);
      client.removeQueries({ queryKey: ["console", scope], predicate: (q) => q.getObserversCount() === 0 });
    },
  });
  return { pending: mutation.isPending, error: mutation.error, start: () => mutation.mutate() };
}
