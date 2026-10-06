// 古い履歴（before_seq）と添付の取得。React の hook と、それが使う純粋な取得手順。

import { useCallback, useEffect, useRef, useState } from "react";
import type { ChatAttachment, ChatMessageListResponse } from "../../../api/generated/types";
import { getAttachment, listMessages, type MessageListParams } from "../data/client";
import type { ChatAction, ChatState } from "../data/reducer";
import { oldestSeq } from "./logic";

export type ListMessages = (
  threadId: string,
  params: MessageListParams,
  signal?: AbortSignal,
) => Promise<ChatMessageListResponse>;

/**
 * 一番古い message より前の頁を取り、history action を返す。
 * 返り値の hasOlder は API の next_before_seq（無ければこれ以上古い頁は無い）。
 */
export async function fetchOlder(
  state: Pick<ChatState, "threadId" | "messages">,
  list: ListMessages = listMessages,
  limit = 50,
  signal?: AbortSignal,
): Promise<{ action: ChatAction | null; hasOlder: boolean }> {
  const before = oldestSeq(state);
  if (before === undefined || before <= 1) return { action: null, hasOlder: false };
  const page = await list(state.threadId, { before_seq: before, limit }, signal);
  const hasOlder = page.next_before_seq !== null && page.next_before_seq !== undefined && page.items.length > 0;
  return { action: page.items.length > 0 ? { type: "history", messages: page.items } : null, hasOlder };
}

/** 古い頁の読み込み状態。最初は一番古い seq が 1 より大きければ「ある」とみなす。 */
export function useOlderMessages(
  state: ChatState,
  dispatch: (action: ChatAction) => void,
  list: ListMessages = listMessages,
) {
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<unknown>(undefined);
  const stateRef = useRef(state);
  stateRef.current = state;
  const threadId = state.threadId;

  // thread を変えたら最初から。
  useEffect(() => {
    void threadId;
    setExhausted(false);
    setError(undefined);
  }, [threadId]);

  const oldest = oldestSeq(state);
  const hasOlder = !exhausted && oldest !== undefined && oldest > 1;

  const loadOlder = useCallback(async () => {
    if (loading) return;
    setLoading(true);
    setError(undefined);
    try {
      const result = await fetchOlder(stateRef.current, list);
      if (result.action) dispatch(result.action);
      if (!result.hasOlder) setExhausted(true);
    } catch (caught) {
      setError(caught);
    } finally {
      setLoading(false);
    }
  }, [dispatch, list, loading]);

  return { hasOlder, loading, error, loadOlder };
}

export type GetAttachment = (id: string, signal?: AbortSignal) => Promise<{ attachment: ChatAttachment }>;

/** 表示中の message が参照する添付をまとめて取る（同じ id は 1 回だけ）。 */
export function useAttachmentMap(ids: string[], get: GetAttachment = getAttachment) {
  const [map, setMap] = useState<Record<string, ChatAttachment | undefined>>({});
  const requested = useRef(new Set<string>());
  const key = ids.join(",");

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    for (const id of key === "" ? [] : key.split(",")) {
      if (requested.current.has(id)) continue;
      requested.current.add(id);
      get(id).then(
        (response) => {
          if (alive.current) setMap((prev) => ({ ...prev, [id]: response.attachment }));
        },
        // 失敗した id は次に一覧が変わったとき取り直す。
        () => requested.current.delete(id),
      );
    }
  }, [key, get]);

  return map;
}

/** state から添付 id を集める（seq 順、重複なし）。 */
export function attachmentIdsOf(state: Pick<ChatState, "messages">): string[] {
  const ids = new Set<string>();
  for (const message of Object.values(state.messages).sort((a, b) => a.seq - b.seq))
    for (const id of message.attachment_ids) ids.add(id);
  return [...ids];
}
