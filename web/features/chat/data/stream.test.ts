import { afterEach, describe, expect, it, vi } from "vitest";
import { delta, detail, event, message, page, run, T } from "./fixtures.test-support";
import { selectActiveRun } from "./reducer";
import { createChatSession } from "./session";
import { createChatStream, frameToEvent, SseParser, type StreamState } from "./stream";

/** 試験側から本文を流せる SSE 応答。 */
class FakeBody {
  private controller!: ReadableStreamDefaultController<Uint8Array>;
  readonly stream = new ReadableStream<Uint8Array>({
    start: (controller) => {
      this.controller = controller;
    },
  });
  send(text: string) {
    this.controller.enqueue(new TextEncoder().encode(text));
  }
  close() {
    this.controller.close();
  }
  fail() {
    this.controller.error(new Error("reset"));
  }
}

type Call = { url: string; headers: Record<string, string>; body?: FakeBody; status: number };

/** 要求ごとに次の status を返す fetch。200 なら FakeBody を付ける。 */
function fakeFetch(statuses: number[]) {
  const calls: Call[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const status = statuses.shift() ?? 200;
    const call: Call = { url: String(input), headers: (init?.headers ?? {}) as Record<string, string>, status };
    calls.push(call);
    if (status !== 200) return new Response(JSON.stringify({ code: "chat-cursor-expired" }), { status });
    call.body = new FakeBody();
    return new Response(call.body.stream, { status, headers: { "Content-Type": "text/event-stream" } });
  }) as unknown as typeof fetch;
  return { fetcher, calls };
}

function sse(ev: ReturnType<typeof event>): string {
  return `event: ${ev.type}\nid: ${ev.id}\ndata: ${JSON.stringify(ev)}\n\n`;
}

/** microtask と fake timer を進めて、条件が満たされるまで待つ（実時間の sleep はしない）。 */
async function until(condition: () => boolean) {
  for (let i = 0; i < 200; i += 1) {
    if (condition()) return;
    await vi.advanceTimersByTimeAsync(0);
  }
  throw new Error("condition not met");
}

afterEach(() => {
  vi.useRealTimers();
});

describe("chat stream", () => {
  it("chat_data_sse_parser_handles_split_chunks_comments_and_crlf", () => {
    const parser = new SseParser();
    expect(parser.push(": heartbeat\n\nevent: queue\r")).toEqual([]);
    expect(parser.push('\nid: 7\r\ndata: {"a":\n')).toEqual([]);
    expect(parser.push("data: 1}\n\n")).toEqual([{ event: "queue", id: "7", data: '{"a":\n1}' }]);
    expect(frameToEvent({ event: "hello", id: "1", data: "{}" })).toBeUndefined();
    expect(frameToEvent({ event: "queue", id: "1", data: "{" })).toBeUndefined();
  });

  it("chat_data_stream_resumes_from_cursor_with_last_event_id_and_after", async () => {
    vi.useFakeTimers();
    const { fetcher, calls } = fakeFetch([200, 200]);
    let cursor = "5";
    const received: string[] = [];
    const stream = createChatStream({
      threadId: T,
      cursor: () => cursor,
      onEvent: (e) => {
        received.push(e.id);
        cursor = e.id;
      },
      onExpired: () => {},
      fetcher,
    });
    stream.start();
    await until(() => calls[0]?.body !== undefined && stream.state() === "open");
    expect(calls[0]?.url).toBe(`/api/chat/threads/${T}/stream?after=5`);
    expect(calls[0]?.headers["Last-Event-ID"]).toBe("5");
    calls[0]?.body?.send(sse(event(6, "queue", { message_ids: [], paused: false })));
    calls[0]?.body?.send(": heartbeat\n\n");
    await until(() => received.length === 1);
    // 切断 → backoff の後、適用済みの cursor から継ぐ。
    calls[0]?.body?.fail();
    await until(() => stream.state() === "reconnecting");
    await vi.advanceTimersByTimeAsync(1_000);
    await until(() => calls.length === 2);
    expect(calls[1]?.url).toBe(`/api/chat/threads/${T}/stream?after=6`);
    expect(calls[1]?.headers["Last-Event-ID"]).toBe("6");
    stream.stop();
  });

  it("chat_data_stream_410_calls_expired_and_does_not_reconnect", async () => {
    vi.useFakeTimers();
    const { fetcher, calls } = fakeFetch([410]);
    const states: StreamState[] = [];
    const onExpired = vi.fn();
    const stream = createChatStream({
      threadId: T,
      cursor: () => "3",
      onEvent: () => {},
      onExpired,
      onState: (s) => states.push(s),
      fetcher,
    });
    stream.start();
    await until(() => onExpired.mock.calls.length === 1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(1);
    expect(states).toContain("expired");
  });

  it("chat_data_session_410_refetches_thread_and_messages_then_resumes", async () => {
    vi.useFakeTimers();
    const { fetcher, calls } = fakeFetch([200, 410, 200]);
    const getThread = vi.fn(async () => detail());
    const listMessages = vi
      .fn()
      .mockResolvedValueOnce(page([message("m1", 1)], "10"))
      .mockResolvedValueOnce(page([message("m1", 1), message("m2", 2)], "50"));
    const session = createChatSession({
      threadId: T,
      api: { getThread, listMessages, postMessage: vi.fn() },
      createStream: (options) => createChatStream({ ...options, fetcher }),
    });
    session.start();
    await until(() => calls.length === 1 && session.getSnapshot().stream === "open");
    expect(calls[0]?.url).toContain("after=10");
    // 切断 → 再接続が 410 → 取り直して snapshot の cursor から継ぐ。
    calls[0]?.body?.close();
    await vi.advanceTimersByTimeAsync(1_000);
    await until(() => calls.length === 3);
    expect(getThread).toHaveBeenCalledTimes(2);
    expect(listMessages).toHaveBeenCalledTimes(2);
    expect(calls[2]?.url).toContain("after=50");
    expect(session.getSnapshot().chat.resync).toBeNull();
    expect(Object.keys(session.getSnapshot().chat.messages)).toEqual(["m1", "m2"]);
    session.stop();
  });

  it("chat_data_session_offset_mismatch_refetches_snapshot", async () => {
    vi.useFakeTimers();
    const { fetcher, calls } = fakeFetch([200, 200]);
    const assistant = message("m2", 2, { role: "assistant", text: "ab", state: "running", run_id: "r1" });
    const listMessages = vi
      .fn()
      .mockResolvedValueOnce(page([assistant], "10"))
      .mockResolvedValueOnce(page([{ ...assistant, text: "abcd" }], "12"));
    const session = createChatSession({
      threadId: T,
      api: { getThread: async () => detail(), listMessages, postMessage: vi.fn() },
      createStream: (options) => createChatStream({ ...options, fetcher }),
    });
    session.start();
    await until(() => session.getSnapshot().stream === "open");
    calls[0]?.body?.send(sse(delta(12, "m2", 3, "d")));
    await until(() => calls.length === 2 && session.getSnapshot().stream === "open");
    expect(listMessages).toHaveBeenCalledTimes(2);
    expect(calls[1]?.url).toContain("after=12");
    expect(session.getSnapshot().chat.messages.m2?.text).toBe("abcd");
    session.stop();
  });

  it("chat_data_disconnect_does_not_mark_run_stopped", async () => {
    vi.useFakeTimers();
    const { fetcher, calls } = fakeFetch([200, 500, 200]);
    const session = createChatSession({
      threadId: T,
      api: {
        getThread: async () => detail({ active_run: run("r1", "running") }),
        listMessages: async () => page([], "10"),
        postMessage: vi.fn(),
      },
      createStream: (options) => createChatStream({ ...options, fetcher }),
    });
    session.start();
    await until(() => session.getSnapshot().stream === "open");
    calls[0]?.body?.fail();
    await until(() => session.getSnapshot().stream === "reconnecting");
    expect(selectActiveRun(session.getSnapshot().chat)?.state).toBe("running");
    await vi.advanceTimersByTimeAsync(1_000);
    await until(() => calls.length === 2);
    await vi.advanceTimersByTimeAsync(2_000);
    await until(() => calls.length === 3 && session.getSnapshot().stream === "open");
    expect(calls[2]?.url).toContain("after=10");
    expect(selectActiveRun(session.getSnapshot().chat)?.state).toBe("running");
    session.stop();
  });
});
