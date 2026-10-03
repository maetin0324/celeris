import type { QueryClient } from "@tanstack/react-query";
import { apiGet, apiMutate } from "../../api/client";
import type { ConsoleBlock, ConsoleInstructAccepted, ConsolePage } from "../../api/generated/types";
import { consoleKeys } from "../../api/queries/keys";
import { appendBlocks } from "./blocks";

// ['console', scope, conversationId] の cache（P3-01）。REST の初回取得と stream の差分を同じ cache に入れる。

export const CONSOLE_PAGE_LIMIT = 100;

export type ConsoleCache = { blocks: ConsoleBlock[]; cursor: string | null };
export const emptyCache: ConsoleCache = { blocks: [], cursor: null };

/** 会話の始まり以降だけを読むための since（新しい会話のとき）。 */
export function consoleQueryKey(scope: string, conversationId: string) {
  return consoleKeys.conversation(scope, conversationId);
}

export function mergePage(cache: ConsoleCache | undefined, page: ConsolePage): ConsoleCache {
  const base = cache ?? emptyCache;
  return { blocks: appendBlocks(page.items, base.blocks), cursor: base.cursor ?? page.next_cursor ?? null };
}

/** stream の block 1 件を cache に足す。cursor は進める。 */
export function applyStreamBlock(client: QueryClient, key: readonly unknown[], block: ConsoleBlock): void {
  client.setQueryData<ConsoleCache>(key, (old) => {
    const base = old ?? emptyCache;
    return { blocks: appendBlocks(base.blocks, [block]), cursor: block.cursor };
  });
}

export function applyStreamHello(client: QueryClient, key: readonly unknown[], cursor: string): void {
  client.setQueryData<ConsoleCache>(key, (old) => {
    const base = old ?? emptyCache;
    return base.cursor === null ? { ...base, cursor } : base;
  });
}

export function fetchConsole(
  client: QueryClient,
  key: readonly unknown[],
  scope: string,
  since: string | null,
  signal?: AbortSignal,
): Promise<ConsoleCache> {
  const params = new URLSearchParams({ scope, limit: String(CONSOLE_PAGE_LIMIT) });
  if (since) params.set("since", since);
  return apiGet<ConsolePage>(`/api/console?${params}`, signal).then((page) =>
    // 取得中に stream が足した block を捨てない。
    mergePage(client.getQueryData<ConsoleCache>(key), page),
  );
}

export type InstructInput = { text: string; scope?: string };

export function sendInstruct(body: InstructInput): Promise<ConsoleInstructAccepted> {
  return apiMutate<ConsoleInstructAccepted>("POST", "/api/console/instruct", body);
}

export function startNewConversation(): Promise<void> {
  return apiMutate<void>("POST", "/api/console/new-conversation", {});
}

/** `text` の先頭が `@<node-id> ` なら scope を付けない（daemon の規則 2 に任せる）。 */
export function buildInstructBody(text: string, scope: string | null): InstructInput {
  if (/^@[^\s@]+\s/.test(text) || !scope || scope === "all") return { text };
  return { text, scope };
}
