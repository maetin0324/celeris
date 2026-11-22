// ADR 2026-11-21-chat-discussion-mode D5・D6・D9: 自分の発言の編集・撤回の呼び出し（expected_revision・409）と、
// 操作を出す発言の判定、SSE の message・message_revised による引用の状態の読み直し。外部ネットワークには出ない。
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApiClient } from "../../../api/client";
import type { ChatCitationView, ChatMessage, ChatMessageRevisedData } from "../../../api/generated/types";
import { editMessage, listMessageRevisions, retractMessage } from "./client";
import { detail, event, message, page, T } from "./fixtures.test-support";
import { type ChatAction, type ChatState, chatReducer, initialChatState } from "./reducer";
import { citationStateNow, revisableBy } from "./revision";
import { createChatSession } from "./session";

type Seen = { url: string; method: string; body: unknown };

function jsonFetch(responses: Array<{ status: number; body?: unknown }>) {
  const seen: Seen[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    });
    const next = responses.shift() ?? { status: 200, body: {} };
    return new Response(next.body === undefined ? "" : JSON.stringify(next.body), { status: next.status });
  }) as unknown as typeof fetch;
  configureApiClient({ fetcher });
  return seen;
}

afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));

const ME = "human:web";
const mine = (over: Partial<ChatMessage> = {}) => message("m1", 1, { text: "仮説 A は誤り", author: ME, ...over });

function cite(messageId: string, revision: number, state: ChatCitationView["state"], thread = T): ChatCitationView {
  return {
    path: "discussion/summary/s.json",
    citation: { thread_id: thread, message_id: messageId, revision, sha256: "a".repeat(64) },
    state,
  };
}

function reply(citations: ChatCitationView[]): ChatMessage {
  return message("m2", 2, {
    role: "assistant",
    text: "要約を添えました",
    citations,
    workspace_files: [{ path: "discussion/summary/s.json", attachment_id: "a1" }],
  });
}

function reduce(actions: ChatAction[], start: ChatState = initialChatState(T)): ChatState {
  return actions.reduce(chatReducer, start);
}

const snapshotOf = (items: ChatMessage[]): ChatAction => ({
  type: "snapshot",
  detail: detail(),
  messages: page(items, "10"),
});

function revised(over: Partial<ChatMessageRevisedData> = {}): ChatMessageRevisedData {
  return {
    message_id: "m1",
    kind: "edit",
    revision: 2,
    previous_revision: 1,
    previous_text: "仮説 A は誤り",
    previous_sha256: "a".repeat(64),
    by: ME,
    ...over,
  };
}

describe("chat_message_revision_client", () => {
  it("chat_message_revision_client_edit_patches_with_expected_revision", async () => {
    const seen = jsonFetch([{ status: 200, body: { message: mine({ revision: 3 }) } }]);
    await editMessage(T, "m1", { text: "仮説 A は条件付きで誤り", expected_revision: 2 });
    expect(seen).toEqual([
      {
        url: "/api/chat/threads/t1/messages/m1",
        method: "PATCH",
        body: { text: "仮説 A は条件付きで誤り", expected_revision: 2 },
      },
    ]);
  });

  it("chat_message_revision_client_retract_posts_with_expected_revision_and_reason", async () => {
    const seen = jsonFetch([{ status: 200, body: { message: mine({ text: "", revision: 2 }) } }]);
    await retractMessage(T, "m1", { expected_revision: 1, reason: "前提が違った" });
    expect(seen).toEqual([
      {
        url: "/api/chat/threads/t1/messages/m1/retract",
        method: "POST",
        body: { expected_revision: 1, reason: "前提が違った" },
      },
    ]);
  });

  it("chat_message_revision_client_revisions_reads_the_previous_bodies", async () => {
    const items = [event(7, "message_revised", revised(), { message_id: "m1" })];
    const seen = jsonFetch([{ status: 200, body: { items, next_cursor: null } }]);
    const response = await listMessageRevisions(T, "m1");
    expect(seen[0]).toMatchObject({ url: "/api/chat/threads/t1/messages/m1/revisions", method: "GET" });
    expect(response.items[0]?.data).toMatchObject({ previous_text: "仮説 A は誤り", previous_revision: 1 });
  });
});

describe("chat_message_revision_session", () => {
  function session(api: Parameters<typeof createChatSession>[0]["api"]) {
    return createChatSession({
      threadId: T,
      api,
      createStream: () => ({ start() {}, stop() {}, reconnect() {}, state: () => "idle" as const }),
    });
  }

  it("chat_message_revision_session_edit_sends_shown_revision_and_shows_the_answer", async () => {
    const editMessage = vi.fn(async () => ({
      message: mine({ text: "新しい本文", revision: 3, edited: { by: ME, at: "t" } }),
    }));
    const s = session({ editMessage });
    s.dispatch(snapshotOf([mine({ revision: 2 })]));
    const outcome = await s.revise(mine({ revision: 2 }), { kind: "edit", text: "新しい本文" });
    expect(editMessage).toHaveBeenCalledWith(T, "m1", { text: "新しい本文", expected_revision: 2 });
    expect(outcome.kind).toBe("ok");
    expect(s.getSnapshot().chat.messages.m1).toMatchObject({ text: "新しい本文", revision: 3 });
  });

  it("chat_message_revision_session_retract_sends_reason_and_expected_revision_1_for_unchanged", async () => {
    const retractMessage = vi.fn(async () => ({
      message: mine({ text: "", revision: 2, retracted: { by: ME, at: "t", reason: "誤り" } }),
    }));
    const s = session({ retractMessage });
    s.dispatch(snapshotOf([mine()]));
    const outcome = await s.revise(mine(), { kind: "retract", reason: "誤り" });
    expect(retractMessage).toHaveBeenCalledWith(T, "m1", { reason: "誤り", expected_revision: 1 });
    expect(outcome.kind).toBe("ok");
    expect(s.getSnapshot().chat.messages.m1?.retracted).toMatchObject({ reason: "誤り" });
  });

  it("chat_message_revision_session_409_is_conflict_and_keeps_the_shown_message", async () => {
    const conflict = new ApiError("conflict", {
      method: "PATCH",
      path: "/api/chat/threads/t1/messages/m1",
      status: 409,
    });
    const s = session({
      editMessage: vi.fn(async () => {
        throw conflict;
      }),
    });
    s.dispatch(snapshotOf([mine()]));
    const outcome = await s.revise(mine(), { kind: "edit", text: "x" });
    expect(outcome.kind).toBe("conflict");
    expect(s.getSnapshot().chat.messages.m1?.text).toBe("仮説 A は誤り");
  });

  it("chat_message_revision_session_403_is_forbidden", async () => {
    const refused = new ApiError("forbidden", { method: "POST", path: "/api/x", status: 403 });
    const s = session({
      retractMessage: vi.fn(async () => {
        throw refused;
      }),
    });
    const outcome = await s.revise(mine(), { kind: "retract", reason: "" });
    expect(outcome.kind).toBe("forbidden");
  });
});

describe("chat_message_revision_who_may_change", () => {
  it("chat_message_revision_own_human_message_offers_edit_and_retract", () => {
    expect(revisableBy(mine(), ME)).toEqual({ edit: true, retract: true });
    expect(revisableBy(mine(), null)).toEqual({ edit: true, retract: true });
    expect(revisableBy(mine({ state: "failed" }), ME)).toEqual({ edit: true, retract: true });
    // 届く前は編集だけ（撤回ではなく取り消しを使う）。run が読んでいる・取り消した発言には出さない。
    expect(revisableBy(mine({ state: "queued" }), ME)).toEqual({ edit: true, retract: false });
    expect(revisableBy(mine({ state: "running" }), ME)).toEqual({ edit: false, retract: false });
    expect(revisableBy(mine({ state: "cancelled" }), ME)).toEqual({ edit: false, retract: false });
  });

  it("chat_message_revision_cos_relay_and_other_people_get_no_actions", () => {
    const none = { edit: false, retract: false };
    expect(revisableBy(mine({ role: "assistant", author: null }), ME)).toEqual(none);
    expect(revisableBy(mine({ role: "system", author: null }), ME)).toEqual(none);
    expect(revisableBy(mine({ author: "operator:claude-code" }), ME)).toEqual(none);
    expect(revisableBy(mine({ author: "mcp:c1" }), ME)).toEqual(none);
    expect(revisableBy(mine({ author: "cos:t0" }), ME)).toEqual(none);
    expect(
      revisableBy(mine({ relay: { relayed_by: "operator:claude-code", said_at: "t", scope: { task_ids: [] } } }), ME),
    ).toEqual(none);
    // 別の人の発言（表示している人が分かっているとき）と、主体の記録の無い発言。
    expect(revisableBy(mine({ author: "human:alice" }), ME)).toEqual(none);
    expect(revisableBy(mine({ author: null }), ME)).toEqual(none);
    expect(revisableBy(mine({ retracted: { by: ME, at: "t" } }), ME)).toEqual(none);
  });

  it("chat_message_revision_viewer_is_learned_from_the_own_send_and_survives_snapshot", () => {
    const pending: ChatAction = { type: "pending_add", clientMessageId: "c-1", text: "x" };
    const ack: ChatAction = {
      type: "pending_ack",
      clientMessageId: "c-1",
      message: mine({ client_message_id: "c-1" }),
    };
    const state = reduce([snapshotOf([]), pending, ack]);
    expect(state.viewer).toBe(ME);
    expect(reduce([snapshotOf([mine()])], state).viewer).toBe(ME);
  });
});

describe("chat_citation_reducer", () => {
  it("chat_citation_message_revised_edit_marks_citing_reply_edited", () => {
    const state = reduce([
      snapshotOf([mine(), reply([cite("m1", 1, "current")])]),
      { type: "event", event: event(11, "message_revised", revised(), { message_id: "m1" }) },
    ]);
    expect(state.messages.m2?.citations?.[0]?.state).toBe("edited");
  });

  it("chat_citation_message_revised_retract_marks_citing_reply_retracted", () => {
    const state = reduce([
      snapshotOf([mine(), reply([cite("m1", 1, "current")])]),
      { type: "event", event: event(11, "message_revised", revised({ kind: "retract" }), { message_id: "m1" }) },
    ]);
    expect(state.messages.m2?.citations?.[0]?.state).toBe("retracted");
  });

  it("chat_citation_message_event_of_cited_message_restates_and_older_revision_is_dropped", () => {
    const edited = mine({ text: "新しい本文", revision: 2, edited: { by: ME, at: "t" } });
    const state = reduce([
      snapshotOf([mine(), reply([cite("m1", 1, "current"), cite("m9", 1, "current", "general")])]),
      { type: "event", event: event(11, "message", { message: edited }, { message_id: "m1" }) },
      // 遅れて届いた古い版の message は表示を戻さない。
      { type: "event", event: event(12, "message", { message: mine() }, { message_id: "m1" }) },
    ]);
    expect(state.messages.m1?.text).toBe("新しい本文");
    expect(state.messages.m2?.citations?.map((view) => view.state)).toEqual(["edited", "current"]);
  });

  it("chat_citation_state_now_follows_the_daemon_order", () => {
    const view = cite("m1", 2, "current");
    expect(citationStateNow(view, { revision: 2, retracted: false })).toBe("current");
    expect(citationStateNow(view, { revision: 3, retracted: false })).toBe("edited");
    expect(citationStateNow(view, { revision: 3, retracted: true })).toBe("retracted");
    expect(citationStateNow(cite("m1", 2, "missing"), { revision: 2, retracted: false })).toBe("missing");
  });
});
