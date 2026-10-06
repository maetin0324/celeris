// ChatEvent を適用する純関数の reducer（ADR 2026-10-05-cos-chat-home D2「SSE: event と再接続」・D5）。
// 規則:
// - event id は thread 内で単調増加する十進文字列。適用済み id 以下は捨てる（再送・重複）。
// - message は id ごとに全置換。text_delta は UTF-8 byte offset が現在本文長と一致するときだけ追記し、
//   不一致なら適用せず resync（thread と messages の取り直し）を求める。
// - tool は call_id、card は kind/id ごとに全置換。queue・thread・run は全置換。
// - 終端の run の後に届いたその run の text_delta・tool・status は捨てる。
// - streaming 中の本文は message と同じ id の 1 項目として持ち、最終 message はそれを置き換える（二重表示しない）。
// - 送信中の人の発言は client_message_id で 1 つだけ持ち、確定した message が来たら消す（再送で吹き出しを増やさない）。

import type {
  ChatCard,
  ChatEvent,
  ChatMessage,
  ChatMessageListResponse,
  ChatQueueData,
  ChatRun,
  ChatRunState,
  ChatStatusData,
  ChatThread,
  ChatThreadDetailResponse,
  ChatToolData,
} from "../../../api/generated/types";

export type ResyncReason = "offset_mismatch" | "cursor_expired";

/** message event が来る前に text_delta だけが届いた返事。message が来たら消す。 */
export type DraftMessage = { id: string; run_id: string | null; text: string };

export type PendingMessage = {
  client_message_id: string;
  text: string;
  attachment_ids: string[];
  mode: "queue" | "interrupt";
  state: "sending" | "failed";
  error?: string;
};

export type ToolEntry = ChatToolData & { run_id: string | null; message_id: string | null };

export type ChatState = {
  threadId: string;
  thread: ChatThread | null;
  /** id → message。表示順は seq（selectTimeline）。 */
  messages: Record<string, ChatMessage>;
  /** message id → 本文の UTF-8 byte 長（offset の照合用）。 */
  textBytes: Record<string, number>;
  drafts: Record<string, DraftMessage>;
  pending: Record<string, PendingMessage>;
  tools: Record<string, ToolEntry>;
  runs: Record<string, ChatRun>;
  /** 稼働中の run の公開要約（終端で消す）。 */
  status: (ChatStatusData & { run_id: string | null }) | null;
  queue: ChatQueueData;
  /** `${kind}:${id}` → card。 */
  cards: Record<string, ChatCard>;
  /** 適用済みの最後の event id（SSE の after）。 */
  lastEventId: string;
  /** null 以外なら thread と messages を取り直す。取り直すまで event を適用しない。 */
  resync: ResyncReason | null;
};

export type ChatAction =
  | { type: "snapshot"; detail: ChatThreadDetailResponse; messages: ChatMessageListResponse }
  | { type: "history"; messages: ChatMessage[] }
  | { type: "event"; event: ChatEvent }
  | { type: "resync_required"; reason: ResyncReason }
  | {
      type: "pending_add";
      clientMessageId: string;
      text: string;
      attachmentIds?: string[];
      mode?: "queue" | "interrupt";
    }
  | { type: "pending_ack"; clientMessageId: string; message: ChatMessage }
  | { type: "pending_failed"; clientMessageId: string; error: string }
  | { type: "pending_discard"; clientMessageId: string };

const TERMINAL_RUN: ReadonlySet<ChatRunState> = new Set(["completed", "stopped", "failed", "interrupted"]);

export function isTerminalRun(state: ChatRunState): boolean {
  return TERMINAL_RUN.has(state);
}

const encoder = new TextEncoder();
export function utf8Length(text: string): number {
  return encoder.encode(text).length;
}

/** 十進文字列の id を数値として比べる（桁数が違っても正しく、Number の精度に依らない）。 */
export function compareEventId(a: string, b: string): number {
  const x = a.replace(/^0+(?=\d)/, "");
  const y = b.replace(/^0+(?=\d)/, "");
  if (x.length !== y.length) return x.length < y.length ? -1 : 1;
  return x < y ? -1 : x > y ? 1 : 0;
}

export function cardKey(card: Pick<ChatCard, "kind" | "id">): string {
  return `${card.kind}:${card.id}`;
}

export function initialChatState(threadId: string): ChatState {
  return {
    threadId,
    thread: null,
    messages: {},
    textBytes: {},
    drafts: {},
    pending: {},
    tools: {},
    runs: {},
    status: null,
    queue: { message_ids: [], paused: false },
    cards: {},
    lastEventId: "0",
    resync: null,
  };
}

function omit<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) return record;
  const next = { ...record };
  delete next[key];
  return next;
}

function withoutPendingFor(pending: Record<string, PendingMessage>, message: ChatMessage) {
  return message.client_message_id ? omit(pending, message.client_message_id) : pending;
}

/** message を全置換し、同じ id の draft と送信中の発言を消す。 */
function putMessage(state: ChatState, message: ChatMessage): ChatState {
  return {
    ...state,
    messages: { ...state.messages, [message.id]: message },
    textBytes: { ...state.textBytes, [message.id]: utf8Length(message.text) },
    drafts: omit(state.drafts, message.id),
    pending: withoutPendingFor(state.pending, message),
  };
}

function isRunClosed(state: ChatState, runId: string | null | undefined): boolean {
  if (!runId) return false;
  const run = state.runs[runId];
  return run !== undefined && isTerminalRun(run.state);
}

function applyTextDelta(state: ChatState, event: ChatEvent, offset: number, text: string): ChatState {
  const messageId = event.message_id;
  if (!messageId) return state;
  const message = state.messages[messageId];
  if (message) {
    const length = state.textBytes[messageId] ?? utf8Length(message.text);
    if (offset !== length) return { ...state, resync: "offset_mismatch" };
    return {
      ...state,
      messages: { ...state.messages, [messageId]: { ...message, text: message.text + text } },
      textBytes: { ...state.textBytes, [messageId]: length + utf8Length(text) },
    };
  }
  const draft = state.drafts[messageId];
  const length = draft ? (state.textBytes[messageId] ?? utf8Length(draft.text)) : 0;
  if (offset !== length) return { ...state, resync: "offset_mismatch" };
  return {
    ...state,
    drafts: {
      ...state.drafts,
      [messageId]: { id: messageId, run_id: event.run_id ?? null, text: (draft?.text ?? "") + text },
    },
    textBytes: { ...state.textBytes, [messageId]: length + utf8Length(text) },
  };
}

function putCard(state: ChatState, card: ChatCard): ChatState {
  const key = cardKey(card);
  let messages = state.messages;
  for (const message of Object.values(state.messages)) {
    if (!message.cards.some((c) => cardKey(c) === key)) continue;
    if (messages === state.messages) messages = { ...state.messages };
    messages[message.id] = { ...message, cards: message.cards.map((c) => (cardKey(c) === key ? card : c)) };
  }
  return { ...state, messages, cards: { ...state.cards, [key]: card } };
}

function applyEventBody(state: ChatState, event: ChatEvent): ChatState {
  const data = event.data as unknown as Record<string, unknown>;
  switch (event.type) {
    case "message":
      return putMessage(state, data.message as ChatMessage);
    case "text_delta": {
      if (isRunClosed(state, event.run_id)) return state;
      return applyTextDelta(state, event, data.offset as number, data.text as string);
    }
    case "status": {
      if (isRunClosed(state, event.run_id)) return state;
      return { ...state, status: { ...(data as unknown as ChatStatusData), run_id: event.run_id ?? null } };
    }
    case "tool": {
      if (isRunClosed(state, event.run_id)) return state;
      const tool = data as unknown as ChatToolData;
      return {
        ...state,
        tools: {
          ...state.tools,
          [tool.call_id]: { ...tool, run_id: event.run_id ?? null, message_id: event.message_id ?? null },
        },
      };
    }
    case "run": {
      const run = data.run as ChatRun;
      const closed = isTerminalRun(run.state);
      return {
        ...state,
        runs: { ...state.runs, [run.id]: run },
        status: closed && state.status?.run_id === run.id ? null : state.status,
      };
    }
    case "queue":
      return { ...state, queue: data as unknown as ChatQueueData };
    case "card":
      return putCard(state, data.card as ChatCard);
    case "thread": {
      const thread = data.thread as ChatThread;
      return thread.id === state.threadId ? { ...state, thread } : state;
    }
    default:
      return state;
  }
}

function applyEvent(state: ChatState, event: ChatEvent): ChatState {
  if (state.resync !== null) return state;
  if (event.thread_id !== state.threadId) return state;
  if (compareEventId(event.id, state.lastEventId) <= 0) return state;
  const next = applyEventBody(state, event);
  // 本文の不一致は適用していないので cursor を進めない（取り直した snapshot の cursor から継ぐ）。
  if (next.resync !== null) return next;
  return { ...next, lastEventId: event.id };
}

function applySnapshot(state: ChatState, detail: ChatThreadDetailResponse, list: ChatMessageListResponse): ChatState {
  const messages: Record<string, ChatMessage> = {};
  const textBytes: Record<string, number> = {};
  let pending = state.pending;
  for (const message of list.items) {
    messages[message.id] = message;
    textBytes[message.id] = utf8Length(message.text);
    pending = withoutPendingFor(pending, message);
  }
  const runs = detail.active_run ? { [detail.active_run.id]: detail.active_run } : {};
  return {
    ...initialChatState(state.threadId),
    thread: detail.thread,
    messages,
    textBytes,
    pending,
    runs,
    cards: {},
    lastEventId: list.snapshot_event_id,
  };
}

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case "snapshot":
      return applySnapshot(state, action.detail, action.messages);
    case "history": {
      // 古い頁を足す。既にある id は新しい側（event で更新済み）を残す。cursor は動かさない。
      const messages = { ...state.messages };
      const textBytes = { ...state.textBytes };
      for (const message of action.messages) {
        if (message.id in messages) continue;
        messages[message.id] = message;
        textBytes[message.id] = utf8Length(message.text);
      }
      return { ...state, messages, textBytes };
    }
    case "event":
      return applyEvent(state, action.event);
    case "resync_required":
      return { ...state, resync: action.reason };
    case "pending_add": {
      const existing = state.pending[action.clientMessageId];
      const acked = Object.values(state.messages).some((m) => m.client_message_id === action.clientMessageId);
      if (acked) return state;
      return {
        ...state,
        pending: {
          ...state.pending,
          [action.clientMessageId]: {
            client_message_id: action.clientMessageId,
            text: existing?.text ?? action.text,
            attachment_ids: existing?.attachment_ids ?? action.attachmentIds ?? [],
            mode: existing?.mode ?? action.mode ?? "queue",
            state: "sending",
          },
        },
      };
    }
    case "pending_ack": {
      const next = putMessage(state, action.message);
      return { ...next, pending: omit(next.pending, action.clientMessageId) };
    }
    case "pending_failed": {
      const pending = state.pending[action.clientMessageId];
      if (!pending) return state;
      return {
        ...state,
        pending: { ...state.pending, [action.clientMessageId]: { ...pending, state: "failed", error: action.error } },
      };
    }
    case "pending_discard":
      return { ...state, pending: omit(state.pending, action.clientMessageId) };
    default:
      return state;
  }
}

export type TimelineItem =
  | { kind: "message"; key: string; message: ChatMessage; streaming: boolean }
  | { kind: "draft"; key: string; draft: DraftMessage }
  | { kind: "pending"; key: string; pending: PendingMessage };

/** 表示順の一覧: 確定・途中の message（seq 順）→ message 未着の streaming 本文 → 送信中の人の発言。 */
export function selectTimeline(state: ChatState): TimelineItem[] {
  const messages = Object.values(state.messages).sort((a, b) => a.seq - b.seq);
  const ackedClientIds = new Set(messages.map((m) => m.client_message_id).filter(Boolean));
  const items: TimelineItem[] = messages.map((message) => ({
    kind: "message",
    key: `m:${message.id}`,
    message,
    streaming: message.state === "running" && !isRunClosed(state, message.run_id),
  }));
  for (const draft of Object.values(state.drafts)) {
    if (draft.id in state.messages) continue;
    items.push({ kind: "draft", key: `m:${draft.id}`, draft });
  }
  for (const pending of Object.values(state.pending)) {
    if (ackedClientIds.has(pending.client_message_id)) continue;
    items.push({ kind: "pending", key: `p:${pending.client_message_id}`, pending });
  }
  return items;
}

/** 稼働中（終端でない）の run。複数あれば新しく届いた方。 */
export function selectActiveRun(state: ChatState): ChatRun | null {
  const active = Object.values(state.runs).filter((run) => !isTerminalRun(run.state));
  if (active.length === 0) return null;
  const byId = state.thread?.active_run_id;
  return active.find((run) => run.id === byId) ?? active[active.length - 1] ?? null;
}
