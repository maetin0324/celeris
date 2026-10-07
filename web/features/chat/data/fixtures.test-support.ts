// 試験用の ChatEvent・message・thread の組み立て。外部ネットワークには出ない。

import type {
  ChatCard,
  ChatEvent,
  ChatEventData,
  ChatEventType,
  ChatMessage,
  ChatMessageListResponse,
  ChatRun,
  ChatRunState,
  ChatThread,
  ChatThreadDetailResponse,
} from "../../../api/generated/types";

export const T = "t1";
const AT = "2026-10-05T00:00:00Z";

export function thread(over: Partial<ChatThread> = {}): ChatThread {
  return {
    id: T,
    kind: "human",
    title: "会話",
    project_id: null,
    status: "open",
    queue_paused: false,
    active_run_id: null,
    queued_count: 0,
    revision: 1,
    created_at: AT,
    updated_at: AT,
    ...over,
  };
}

export function message(id: string, seq: number, over: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id,
    thread_id: T,
    seq,
    role: "user",
    text: "",
    state: "completed",
    client_message_id: null,
    reply_to_id: null,
    run_id: null,
    attachment_ids: [],
    cards: [],
    created_at: AT,
    updated_at: AT,
    ...over,
  };
}

export function run(id: string, state: ChatRunState, over: Partial<ChatRun> = {}): ChatRun {
  return { id, thread_id: T, input_message_id: "m1", output_message_id: "m2", state, ...over };
}

export function card(over: Partial<ChatCard> = {}): ChatCard {
  return {
    kind: "decision",
    id: "d1",
    title: "決定",
    state: "pending",
    href: "/inbox",
    actor: "system",
    reason: null,
    operation_id: null,
    ...over,
  };
}

export function event(
  id: number | string,
  type: ChatEventType,
  data: unknown,
  over: { run_id?: string | null; message_id?: string | null; thread_id?: string } = {},
): ChatEvent {
  return {
    id: String(id),
    type,
    thread_id: over.thread_id ?? T,
    run_id: over.run_id ?? null,
    message_id: over.message_id ?? null,
    at: AT,
    data: data as ChatEventData,
  };
}

export function delta(id: number, messageId: string, offset: number, text: string, runId = "r1"): ChatEvent {
  return event(id, "text_delta", { offset, text }, { run_id: runId, message_id: messageId });
}

export function detail(over: Partial<ChatThreadDetailResponse> = {}): ChatThreadDetailResponse {
  return { thread: thread(), active_run: null, last_event_id: "0", ...over };
}

export function page(items: ChatMessage[], snapshotEventId: string): ChatMessageListResponse {
  return { items, next_before_seq: null, next_after_seq: null, snapshot_event_id: snapshotEventId };
}
