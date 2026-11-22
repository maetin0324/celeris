// ADR 2026-11-21-chat-discussion-mode D1.1・D9: 送信の本文の discussion_mode。
// 守る不変条件: 議論・案を選んだ送信だけが欄を運び、「実行」と未選択の送信は欄を持たない（mode を選ぶ前と同じ本文）。
// 失敗した送信の再送も最初の扱いを保つ（議論の発言が再送で実行に緩まない）。既存の client・session の試験は
// discussion_mode を知らないので、欄の有無も再送での保持も検出できない。
import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApiClient } from "../../../api/client";
import type { ChatPostMessageRequest, ChatPostMessageResponse } from "../../../api/generated/types";
import { postMessage } from "./client";
import { message, T } from "./fixtures.test-support";
import { chatReducer, initialChatState } from "./reducer";
import { discussionModeField } from "./send";
import { createChatSession } from "./session";

const BASE_KEYS = ["attachment_ids", "client_message_id", "mode", "reply_to_id", "resume_queue", "text"];

function capture() {
  const bodies: ChatPostMessageRequest[] = [];
  const session = createChatSession({
    threadId: T,
    api: {
      getThread: vi.fn(),
      listMessages: vi.fn(),
      postMessage: async (_thread, body): Promise<ChatPostMessageResponse> => {
        bodies.push(body);
        return {
          message: message("m1", 1, { client_message_id: body.client_message_id, text: body.text }),
          queue_position: 0,
        };
      },
    },
  });
  return { session, bodies };
}

afterEach(() => {
  configureApiClient({ fetcher: (input, init) => fetch(input, init) });
});

describe("chat_discussion_mode_send", () => {
  it("chat_discussion_mode_send_body_carries_discuss_and_propose", async () => {
    const { session, bodies } = capture();
    await session.send({ client_message_id: "c1", text: "仮説を批判して", discussion_mode: "discuss" });
    await session.send({ client_message_id: "c2", text: "案にして", discussion_mode: "propose" });
    expect(bodies.map((body) => body.discussion_mode)).toEqual(["discuss", "propose"]);
    expect(Object.keys(bodies[0] ?? {}).sort()).toEqual([...BASE_KEYS, "discussion_mode"].sort());
  });

  it("chat_discussion_mode_send_default_execute_omits_the_field", async () => {
    const { session, bodies } = capture();
    await session.send({ client_message_id: "c1", text: "直して" });
    await session.send({ client_message_id: "c2", text: "直して", discussion_mode: "execute" });
    await session.send({ client_message_id: "c3", text: "直して", discussion_mode: null });
    for (const body of bodies) {
      expect("discussion_mode" in body).toBe(false);
      expect(Object.keys(body).sort()).toEqual(BASE_KEYS);
    }
    expect(bodies[0]).toStrictEqual({
      client_message_id: "c1",
      text: "直して",
      attachment_ids: [],
      reply_to_id: null,
      mode: "queue",
      resume_queue: false,
    });
    expect(discussionModeField("execute")).toStrictEqual({});
    expect(discussionModeField(undefined)).toStrictEqual({});
  });

  it("chat_discussion_mode_client_puts_the_field_on_the_wire_only_when_set", async () => {
    const seen: unknown[] = [];
    configureApiClient({
      fetcher: (async (_input: RequestInfo | URL, init?: RequestInit) => {
        seen.push(typeof init?.body === "string" ? JSON.parse(init.body) : undefined);
        return new Response(JSON.stringify({ message: message("m1", 1), queue_position: 0 }), { status: 202 });
      }) as typeof fetch,
    });
    const base: ChatPostMessageRequest = {
      client_message_id: "c1",
      text: "x",
      attachment_ids: [],
      mode: "queue",
      resume_queue: false,
    };
    await postMessage(T, { ...base, ...discussionModeField("discuss") });
    await postMessage(T, { ...base, ...discussionModeField("execute") });
    expect((seen[0] as Record<string, unknown>).discussion_mode).toBe("discuss");
    expect(Object.hasOwn(seen[1] as object, "discussion_mode")).toBe(false);
  });

  it("chat_discussion_mode_failed_send_keeps_the_mode_for_retry", async () => {
    const session = createChatSession({
      threadId: T,
      api: {
        getThread: vi.fn(),
        listMessages: vi.fn(),
        postMessage: async () => {
          throw new Error("rejected");
        },
      },
      sendRetry: { attempts: 1 },
    });
    await expect(
      session.send({ client_message_id: "c1", text: "議論したい", discussion_mode: "discuss" }),
    ).rejects.toThrow("rejected");
    expect(session.getSnapshot().chat.pending.c1).toMatchObject({ state: "failed", discussion_mode: "discuss" });
    // 同じ client_message_id の 2 度目の pending_add は、最初の扱いを保つ（再送で緩めない）。
    const again = chatReducer(session.getSnapshot().chat, {
      type: "pending_add",
      clientMessageId: "c1",
      text: "議論したい",
    });
    expect(again.pending.c1?.discussion_mode).toBe("discuss");
    const plain = chatReducer(initialChatState(T), { type: "pending_add", clientMessageId: "c2", text: "x" });
    expect("discussion_mode" in (plain.pending.c2 ?? {})).toBe(false);
  });
});
