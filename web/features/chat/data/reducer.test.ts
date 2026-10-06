import { describe, expect, it } from "vitest";
import { card, delta, detail, event, message, page, run, T, thread } from "./fixtures.test-support";
import {
  type ChatAction,
  type ChatState,
  chatReducer,
  compareEventId,
  initialChatState,
  selectActiveRun,
  selectTimeline,
} from "./reducer";

function reduce(actions: ChatAction[], state: ChatState = initialChatState(T)): ChatState {
  return actions.reduce(chatReducer, state);
}

function fromSnapshot(messages = [message("m1", 1, { text: "やって", client_message_id: "c1" })], cursor = "10") {
  return reduce([{ type: "snapshot", detail: detail(), messages: page(messages, cursor) }]);
}

describe("chat reducer", () => {
  it("chat_data_compare_event_id_is_numeric_not_lexical", () => {
    expect(compareEventId("9", "10")).toBe(-1);
    expect(compareEventId("100", "99")).toBe(1);
    expect(compareEventId("0012", "12")).toBe(0);
    expect(compareEventId("123456789012345678901", "123456789012345678900")).toBe(1);
  });

  it("chat_data_snapshot_sets_cursor_from_snapshot_event_id", () => {
    const state = fromSnapshot(undefined, "42");
    expect(state.lastEventId).toBe("42");
    expect(state.thread?.id).toBe(T);
    expect(selectTimeline(state).map((i) => i.key)).toEqual(["m:m1"]);
  });

  it("chat_data_drops_events_with_id_at_or_below_applied", () => {
    const base = fromSnapshot(undefined, "10");
    const msg = message("m2", 2, { role: "assistant", text: "古い" });
    // snapshot の cursor 以下は捨てる。
    const s1 = chatReducer(base, { type: "event", event: event(10, "message", { message: msg }) });
    expect(s1).toBe(base);
    const s2 = chatReducer(base, { type: "event", event: event(11, "message", { message: { ...msg, text: "新" } }) });
    expect(s2.lastEventId).toBe("11");
    // 同じ id の再送・逆順は捨てる。
    const s3 = reduce(
      [
        { type: "event", event: event(11, "message", { message: { ...msg, text: "重複" } }) },
        { type: "event", event: event(9, "message", { message: { ...msg, text: "逆順" } }) },
      ],
      s2,
    );
    expect(s3).toBe(s2);
    expect(s3.messages.m2?.text).toBe("新");
  });

  it("chat_data_ignores_events_of_other_threads", () => {
    const base = fromSnapshot();
    const next = chatReducer(base, {
      type: "event",
      event: event(11, "message", { message: message("x", 1) }, { thread_id: "other" }),
    });
    expect(next).toBe(base);
  });

  it("chat_data_message_event_replaces_whole_message_by_id", () => {
    const base = fromSnapshot();
    const next = reduce(
      [
        {
          type: "event",
          event: event(11, "message", { message: message("m1", 1, { text: "やって", state: "running" }) }),
        },
        { type: "event", event: event(12, "card", { card: card({ id: "d9" }) }) },
        {
          type: "event",
          event: event(13, "message", { message: message("m1", 1, { text: "置換", state: "completed", cards: [] }) }),
        },
      ],
      base,
    );
    expect(next.messages.m1).toMatchObject({ text: "置換", state: "completed", client_message_id: null });
    expect(Object.keys(next.messages)).toEqual(["m1"]);
  });

  it("chat_data_text_delta_appends_only_when_offset_matches_utf8_length", () => {
    const base = fromSnapshot([
      message("m1", 1, { text: "やって" }),
      message("m2", 2, { role: "assistant", text: "", state: "running", run_id: "r1" }),
    ]);
    const next = reduce(
      [
        { type: "event", event: delta(11, "m2", 0, "確認") },
        // 「確認」は UTF-8 で 6 byte。
        { type: "event", event: delta(12, "m2", 6, "します🙂") },
      ],
      base,
    );
    expect(next.messages.m2?.text).toBe("確認します🙂");
    expect(next.textBytes.m2).toBe(6 + 9 + 4);
    expect(next.resync).toBeNull();
    expect(next.lastEventId).toBe("12");
  });

  it("chat_data_text_delta_offset_mismatch_requests_resync_without_applying", () => {
    const base = fromSnapshot([message("m2", 2, { role: "assistant", text: "abc", state: "running", run_id: "r1" })]);
    const gap = chatReducer(base, { type: "event", event: delta(11, "m2", 5, "xyz") });
    expect(gap.resync).toBe("offset_mismatch");
    expect(gap.messages.m2?.text).toBe("abc");
    // cursor は進めない（取り直した snapshot から継ぐ）。
    expect(gap.lastEventId).toBe("10");
    // 取り直すまでは何も適用しない。
    const held = chatReducer(gap, { type: "event", event: delta(12, "m2", 3, "d") });
    expect(held).toBe(gap);
    // snapshot で解除される。
    const fixed = chatReducer(held, {
      type: "snapshot",
      detail: detail(),
      messages: page([message("m2", 2, { role: "assistant", text: "abcxyz", state: "running", run_id: "r1" })], "12"),
    });
    expect(fixed.resync).toBeNull();
    expect(fixed.lastEventId).toBe("12");
    expect(chatReducer(fixed, { type: "event", event: delta(13, "m2", 6, "!") }).messages.m2?.text).toBe("abcxyz!");
  });

  it("chat_data_cursor_expired_410_requests_resync", () => {
    const base = fromSnapshot();
    const next = chatReducer(base, { type: "resync_required", reason: "cursor_expired" });
    expect(next.resync).toBe("cursor_expired");
    expect(chatReducer(next, { type: "event", event: event(11, "queue", { message_ids: [], paused: true }) })).toBe(
      next,
    );
  });

  it("chat_data_streaming_draft_and_final_message_are_not_shown_twice", () => {
    const base = fromSnapshot();
    const streaming = reduce(
      [
        { type: "event", event: delta(11, "m2", 0, "途中") },
        { type: "event", event: delta(12, "m2", 6, "の本文") },
      ],
      base,
    );
    expect(selectTimeline(streaming).map((i) => i.key)).toEqual(["m:m1", "m:m2"]);
    expect(selectTimeline(streaming)[1]).toMatchObject({ kind: "draft", draft: { text: "途中の本文" } });
    const final = reduce(
      [
        {
          type: "event",
          event: event(
            13,
            "message",
            { message: message("m2", 2, { role: "assistant", text: "最終の本文", state: "completed", run_id: "r1" }) },
            { run_id: "r1", message_id: "m2" },
          ),
        },
        { type: "event", event: event(14, "run", { run: run("r1", "completed") }, { run_id: "r1" }) },
      ],
      streaming,
    );
    const timeline = selectTimeline(final);
    expect(timeline.map((i) => i.key)).toEqual(["m:m1", "m:m2"]);
    expect(timeline[1]).toMatchObject({ kind: "message", streaming: false, message: { text: "最終の本文" } });
    expect(final.drafts).toEqual({});
  });

  it("chat_data_drops_body_deltas_after_run_terminal", () => {
    const base = fromSnapshot([message("m2", 2, { role: "assistant", text: "完", state: "running", run_id: "r1" })]);
    const ended = reduce(
      [
        { type: "event", event: event(11, "status", { phase: "working", summary: "作業中" }, { run_id: "r1" }) },
        { type: "event", event: event(12, "run", { run: run("r1", "stopped") }, { run_id: "r1" }) },
      ],
      base,
    );
    expect(ended.status).toBeNull();
    expect(selectActiveRun(ended)).toBeNull();
    const late = reduce(
      [
        { type: "event", event: delta(13, "m2", 3, "後から") },
        {
          type: "event",
          event: event(
            14,
            "tool",
            { call_id: "c1", name: "Bash", state: "running", summary: "", error: false, truncated: false },
            { run_id: "r1" },
          ),
        },
        { type: "event", event: event(15, "status", { phase: "thinking", summary: "x" }, { run_id: "r1" }) },
      ],
      ended,
    );
    expect(late.messages.m2?.text).toBe("完");
    expect(late.tools).toEqual({});
    expect(late.status).toBeNull();
    expect(late.resync).toBeNull();
    expect(late.lastEventId).toBe("15");
  });

  it("chat_data_tool_updates_by_call_id", () => {
    const base = fromSnapshot();
    const tool = { call_id: "call-1", name: "Bash", summary: "確認", detail: null, error: false, truncated: false };
    const next = reduce(
      [
        { type: "event", event: event(11, "tool", { ...tool, state: "running" }, { run_id: "r1", message_id: "m2" }) },
        {
          type: "event",
          event: event(12, "tool", { ...tool, state: "failed", error: true }, { run_id: "r1", message_id: "m2" }),
        },
        {
          type: "event",
          event: event(13, "tool", { ...tool, call_id: "call-2", state: "completed" }, { run_id: "r1" }),
        },
      ],
      base,
    );
    expect(Object.keys(next.tools).sort()).toEqual(["call-1", "call-2"]);
    expect(next.tools["call-1"]).toMatchObject({ state: "failed", error: true, run_id: "r1", message_id: "m2" });
  });

  it("chat_data_queue_card_thread_run_are_replaced_whole", () => {
    const base = fromSnapshot([message("m1", 1, { cards: [card({ state: "pending" })] })]);
    const next = reduce(
      [
        { type: "event", event: event(11, "queue", { message_ids: ["m3", "m4"], paused: false }) },
        { type: "event", event: event(12, "queue", { message_ids: ["m4"], paused: true }) },
        { type: "event", event: event(13, "card", { card: card({ state: "answered", title: "済" }) }) },
        { type: "event", event: event(14, "thread", { thread: thread({ title: "新題", revision: 2 }) }) },
        { type: "event", event: event(15, "run", { run: run("r1", "running") }, { run_id: "r1" }) },
        { type: "event", event: event(16, "run", { run: run("r1", "stopping", { reason: "停止要求" }) }) },
      ],
      base,
    );
    expect(next.queue).toEqual({ message_ids: ["m4"], paused: true });
    expect(next.cards["decision:d1"]).toMatchObject({ state: "answered", title: "済" });
    expect(next.messages.m1?.cards[0]).toMatchObject({ state: "answered", title: "済" });
    expect(next.thread).toMatchObject({ title: "新題", revision: 2 });
    expect(next.runs.r1).toMatchObject({ state: "stopping", reason: "停止要求" });
    expect(selectActiveRun(next)?.id).toBe("r1");
  });

  it("chat_data_resend_with_same_client_message_id_does_not_add_bubbles", () => {
    const base = fromSnapshot([]);
    const sent = reduce(
      [
        { type: "pending_add", clientMessageId: "c9", text: "直して" },
        { type: "pending_failed", clientMessageId: "c9", error: "network" },
        { type: "pending_add", clientMessageId: "c9", text: "直して" },
        { type: "pending_add", clientMessageId: "c9", text: "直して" },
      ],
      base,
    );
    expect(selectTimeline(sent)).toHaveLength(1);
    expect(sent.pending.c9?.state).toBe("sending");
    // 応答より先に SSE の message が届いても、後から応答が届いても 1 つ。
    const echoed = chatReducer(sent, {
      type: "event",
      event: event(11, "message", { message: message("m5", 5, { client_message_id: "c9", state: "queued" }) }),
    });
    expect(selectTimeline(echoed).map((i) => i.key)).toEqual(["m:m5"]);
    const acked = chatReducer(echoed, {
      type: "pending_ack",
      clientMessageId: "c9",
      message: message("m5", 5, { client_message_id: "c9", state: "queued" }),
    });
    expect(selectTimeline(acked).map((i) => i.key)).toEqual(["m:m5"]);
    // 確定後に同じ id を pending_add しても増えない。
    expect(
      selectTimeline(chatReducer(acked, { type: "pending_add", clientMessageId: "c9", text: "直して" })),
    ).toHaveLength(1);
  });

  it("chat_data_snapshot_drops_pending_already_stored", () => {
    const pending = reduce([{ type: "pending_add", clientMessageId: "c1", text: "やって" }]);
    const state = chatReducer(pending, {
      type: "snapshot",
      detail: detail(),
      messages: page([message("m1", 1, { client_message_id: "c1" })], "3"),
    });
    expect(state.pending).toEqual({});
    expect(selectTimeline(state)).toHaveLength(1);
  });

  it("chat_data_history_prepends_without_moving_cursor", () => {
    const base = fromSnapshot([message("m5", 5, { text: "新" })], "20");
    const next = chatReducer(base, {
      type: "history",
      messages: [message("m3", 3), message("m5", 5, { text: "古い頁の値" })],
    });
    expect(selectTimeline(next).map((i) => i.key)).toEqual(["m:m3", "m:m5"]);
    expect(next.messages.m5?.text).toBe("新");
    expect(next.lastEventId).toBe("20");
  });
});
