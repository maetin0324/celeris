// 1 thread の表示状態を持つ controller（React に依らない。hook は use-chat-session.ts）。
// 流れ: thread と messages の snapshot → reducer に入れる → snapshot_event_id から stream を継ぐ。
// text_delta の offset 不一致（reducer の resync）と 410（cursor 失効）は、stream を止めて snapshot から取り直す。
// stream の切断・再接続は run の状態を変えない（停止は run event か stop API の結果だけで表す）。

import type {
  ChatMessageListResponse,
  ChatPostMessageRequest,
  ChatPostMessageResponse,
  ChatThreadDetailResponse,
} from "../../../api/generated/types";
import { getThread, listMessages, postMessage } from "./client";
import { type ChatAction, type ChatState, chatReducer, initialChatState } from "./reducer";
import { sendWithRetry } from "./send";
import { type ChatStream, type ChatStreamOptions, createChatStream, type StreamState } from "./stream";

export type ChatSessionApi = {
  getThread: (threadId: string, signal?: AbortSignal) => Promise<ChatThreadDetailResponse>;
  listMessages: (threadId: string, signal?: AbortSignal) => Promise<ChatMessageListResponse>;
  postMessage: (threadId: string, body: ChatPostMessageRequest) => Promise<ChatPostMessageResponse>;
};

export const defaultChatSessionApi: ChatSessionApi = {
  getThread: (threadId, signal) => getThread(threadId, signal),
  listMessages: (threadId, signal) => listMessages(threadId, {}, signal),
  postMessage: (threadId, body) => postMessage(threadId, body),
};

export type ChatSessionSnapshot = {
  chat: ChatState;
  stream: StreamState;
  /** 最後の snapshot 取得の失敗（再試行は refresh()）。 */
  loadError: unknown;
  loading: boolean;
};

export type ChatSessionOptions = {
  threadId: string;
  api?: ChatSessionApi;
  createStream?: (options: ChatStreamOptions) => ChatStream;
  /** 送信の再試行の待ち（既定 send.ts）。 */
  sendRetry?: { attempts?: number; delayMs?: number };
};

export type SendInput = Omit<ChatPostMessageRequest, "mode" | "resume_queue" | "attachment_ids"> & {
  attachment_ids?: string[];
  mode?: ChatPostMessageRequest["mode"];
  resume_queue?: boolean;
};

export type ChatSession = {
  start(): void;
  stop(): void;
  /** snapshot から取り直す（resync・410・手動の再読込）。 */
  refresh(): Promise<void>;
  /** 復帰時。張り直しだけ（cursor から継ぐ）。 */
  reconnect(): void;
  dispatch(action: ChatAction): void;
  /** client_message_id を固定して送る。失敗後に同じ id で呼べば吹き出しは増えない。 */
  send(input: SendInput): Promise<ChatPostMessageResponse>;
  getSnapshot(): ChatSessionSnapshot;
  subscribe(listener: () => void): () => void;
};

export function createChatSession(options: ChatSessionOptions): ChatSession {
  const api = options.api ?? defaultChatSessionApi;
  const listeners = new Set<() => void>();
  let snapshot: ChatSessionSnapshot = {
    chat: initialChatState(options.threadId),
    stream: "idle",
    loadError: undefined,
    loading: false,
  };
  let running = false;
  let loadGeneration = 0;
  let loadController: AbortController | undefined;

  function publish(next: Partial<ChatSessionSnapshot>) {
    snapshot = { ...snapshot, ...next };
    for (const listener of listeners) listener();
  }

  function dispatch(action: ChatAction) {
    const chat = chatReducer(snapshot.chat, action);
    if (chat === snapshot.chat) return;
    const resyncStarted = chat.resync !== null && snapshot.chat.resync === null;
    publish({ chat });
    if (resyncStarted && running) void refresh();
  }

  const stream = (options.createStream ?? createChatStream)({
    threadId: options.threadId,
    cursor: () => snapshot.chat.lastEventId,
    onEvent: (event) => dispatch({ type: "event", event }),
    onExpired: () => {
      dispatch({ type: "resync_required", reason: "cursor_expired" });
    },
    onState: (state) => publish({ stream: state }),
  });

  async function refresh() {
    stream.stop();
    loadController?.abort();
    const controller = new AbortController();
    loadController = controller;
    const generation = ++loadGeneration;
    publish({ loading: true, loadError: undefined });
    try {
      const [detail, messages] = await Promise.all([
        api.getThread(options.threadId, controller.signal),
        api.listMessages(options.threadId, controller.signal),
      ]);
      if (generation !== loadGeneration || !running) return;
      const chat = chatReducer(snapshot.chat, { type: "snapshot", detail, messages });
      publish({ chat, loading: false });
      stream.start();
    } catch (error) {
      if (generation !== loadGeneration || !running) return;
      publish({ loading: false, loadError: error });
    }
  }

  return {
    start() {
      if (running) return;
      running = true;
      void refresh();
    },
    stop() {
      running = false;
      loadGeneration += 1;
      loadController?.abort();
      stream.stop();
    },
    refresh,
    reconnect() {
      if (!running || snapshot.chat.resync !== null || snapshot.loading) return;
      stream.reconnect();
    },
    dispatch,
    async send(input) {
      const body: ChatPostMessageRequest = {
        client_message_id: input.client_message_id,
        text: input.text,
        attachment_ids: input.attachment_ids ?? [],
        reply_to_id: input.reply_to_id ?? null,
        mode: input.mode ?? "queue",
        resume_queue: input.resume_queue ?? false,
      };
      dispatch({
        type: "pending_add",
        clientMessageId: body.client_message_id,
        text: body.text,
        attachmentIds: body.attachment_ids,
        mode: body.mode,
      });
      try {
        const response = await sendWithRetry(() => api.postMessage(options.threadId, body), options.sendRetry);
        dispatch({ type: "pending_ack", clientMessageId: body.client_message_id, message: response.message });
        return response;
      } catch (error) {
        dispatch({
          type: "pending_failed",
          clientMessageId: body.client_message_id,
          error: error instanceof Error ? error.message : String(error),
        });
        throw error;
      }
    },
    getSnapshot: () => snapshot,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
