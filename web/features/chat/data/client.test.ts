import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApiClient } from "../../../api/client";
import {
  answerDecision,
  answerPlanGate,
  cancelMessage,
  decideApproval,
  listMessages,
  listThreads,
  overrideOperation,
  postMessage,
  resumeQueue,
  stopRun,
  uploadAttachment,
  type XhrLike,
} from "./client";
import { message, T } from "./fixtures.test-support";
import { sendWithRetry } from "./send";
import { createChatSession } from "./session";

type Seen = { url: string; method: string; body: unknown };

function jsonFetch(responses: Array<{ status: number; body?: unknown } | Error>) {
  const seen: Seen[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    });
    const next = responses.shift() ?? { status: 200, body: {} };
    if (next instanceof Error) throw next;
    return new Response(next.body === undefined ? "" : JSON.stringify(next.body), { status: next.status });
  }) as unknown as typeof fetch;
  configureApiClient({ fetcher });
  return seen;
}

class FakeXhr implements XhrLike {
  static last: FakeXhr | undefined;
  method = "";
  url = "";
  headers: Record<string, string> = {};
  sent: FormData | undefined;
  status = 0;
  responseText = "";
  withCredentials = false;
  aborted = false;
  upload: XhrLike["upload"] = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  ontimeout: (() => void) | null = null;
  constructor() {
    FakeXhr.last = this;
  }
  open(method: string, url: string) {
    this.method = method;
    this.url = url;
  }
  setRequestHeader(name: string, value: string) {
    this.headers[name] = value;
  }
  send(body: FormData) {
    this.sent = body;
  }
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
  respond(status: number, body: unknown) {
    this.status = status;
    this.responseText = JSON.stringify(body);
    this.onload?.();
  }
}

beforeEach(() => {
  FakeXhr.last = undefined;
});

afterEach(() => {
  configureApiClient({ fetcher: (input, init) => fetch(input, init) });
  vi.useRealTimers();
});

const attachment = {
  id: "a1",
  thread_id: T,
  name: "screen.png",
  media_type: "image/png",
  size_bytes: 3,
  sha256: "0".repeat(64),
  state: "ready",
  preview_url: null,
  download_url: "/api/v1/chat/attachments/a1/content",
  expires_at: null,
};

describe("chat client", () => {
  it("chat_data_client_paths_and_bodies_follow_the_api_contract", async () => {
    const seen = jsonFetch([]);
    await listThreads({ q: "画面", status: "open", limit: 50 });
    await listMessages("t/1", { before_seq: 100, limit: 50 });
    await postMessage(T, {
      client_message_id: "c1",
      text: "直して",
      attachment_ids: ["a1"],
      reply_to_id: null,
      mode: "queue",
      resume_queue: false,
    });
    await cancelMessage(T, "m3");
    await stopRun(T, "r1");
    await resumeQueue(T, 2);
    await answerDecision("d1", { option: "continue", note: null });
    await decideApproval("ap1", { decision: "once", answer: "ok" });
    await answerPlanGate("task1", { action: "approve" });
    await overrideOperation("o1", { action: "return", reason: "再検討" });
    expect(seen.map((s) => `${s.method} ${s.url}`)).toEqual([
      "GET /api/v1/chat/threads?q=%E7%94%BB%E9%9D%A2&status=open&limit=50",
      "GET /api/v1/chat/threads/t%2F1/messages?before_seq=100&limit=50",
      "POST /api/v1/chat/threads/t1/messages",
      "DELETE /api/v1/chat/threads/t1/messages/m3",
      "POST /api/v1/chat/threads/t1/stop",
      "POST /api/v1/chat/threads/t1/resume-queue",
      "POST /api/v1/decisions/d1/answer",
      "POST /api/v1/approvals/ap1/decide",
      "POST /api/v1/tasks/task1/execution/plan-gate",
      "POST /api/v1/cos/operations/o1/override",
    ]);
    expect(seen[4]?.body).toEqual({ run_id: "r1" });
    expect(seen[5]?.body).toEqual({ expected_revision: 2 });
    expect(() => listMessages(T, { before_seq: 1, after_seq: 2 })).toThrow(TypeError);
  });

  it("chat_data_upload_reports_progress_and_returns_attachment", async () => {
    const progress: Array<number | undefined> = [];
    const promise = uploadAttachment(T, new File(["abc"], "screen.png", { type: "image/png" }), {
      clientUploadId: "u1",
      onProgress: (p) => progress.push(p.total === undefined ? undefined : p.loaded / p.total),
      createXhr: () => new FakeXhr(),
    });
    const xhr = FakeXhr.last;
    expect(xhr?.method).toBe("POST");
    expect(xhr?.url).toBe(`/api/v1/chat/threads/${T}/attachments`);
    expect(xhr?.sent?.get("client_upload_id")).toBe("u1");
    expect((xhr?.sent?.get("file") as File | undefined)?.name).toBe("screen.png");
    xhr?.upload.onprogress?.({ loaded: 1, total: 2, lengthComputable: true });
    xhr?.upload.onprogress?.({ loaded: 2, total: 0, lengthComputable: false });
    xhr?.respond(201, { attachment });
    await expect(promise).resolves.toEqual(attachment);
    expect(progress).toEqual([0.5, undefined]);
  });

  it("chat_data_upload_abort_and_http_errors_are_api_errors", async () => {
    const controller = new AbortController();
    const aborted = uploadAttachment(T, new Blob(["x"]), {
      clientUploadId: "u2",
      signal: controller.signal,
      createXhr: () => new FakeXhr(),
    });
    controller.abort();
    await expect(aborted).rejects.toMatchObject({ kind: "aborted" });
    expect(FakeXhr.last?.aborted).toBe(true);

    const tooLarge = uploadAttachment(T, new Blob(["x"]), { clientUploadId: "u3", createXhr: () => new FakeXhr() });
    FakeXhr.last?.respond(413, { title: "too large" });
    await expect(tooLarge).rejects.toBeInstanceOf(ApiError);
    await expect(tooLarge).rejects.toMatchObject({ status: 413 });

    const pre = new AbortController();
    pre.abort();
    await expect(
      uploadAttachment(T, new Blob(["x"]), {
        clientUploadId: "u4",
        signal: pre.signal,
        createXhr: () => new FakeXhr(),
      }),
    ).rejects.toMatchObject({ kind: "aborted" });
  });
});

describe("chat send", () => {
  it("chat_data_send_retries_with_same_client_message_id_and_one_bubble", async () => {
    vi.useFakeTimers();
    const seen = jsonFetch([
      new TypeError("offline"),
      { status: 502, body: { title: "bad gateway" } },
      {
        status: 202,
        body: {
          message: message("m7", 7, { client_message_id: "c7", text: "直して", state: "queued" }),
          queue_position: 1,
        },
      },
    ]);
    const session = createChatSession({
      threadId: T,
      api: {
        getThread: vi.fn(),
        listMessages: vi.fn(),
        postMessage: (threadId, body) => postMessage(threadId, body),
      },
      sendRetry: { attempts: 3, delayMs: 100 },
    });
    const sending = session.send({ client_message_id: "c7", text: "直して" });
    expect(session.getSnapshot().chat.pending.c7?.state).toBe("sending");
    await vi.advanceTimersByTimeAsync(100);
    await vi.advanceTimersByTimeAsync(200);
    await sending;
    expect(seen).toHaveLength(3);
    expect(new Set(seen.map((s) => (s.body as { client_message_id: string }).client_message_id))).toEqual(
      new Set(["c7"]),
    );
    const chat = session.getSnapshot().chat;
    expect(chat.pending).toEqual({});
    expect(Object.keys(chat.messages)).toEqual(["m7"]);
  });

  it("chat_data_send_does_not_retry_conflict_and_marks_failed_once", async () => {
    const seen = jsonFetch([{ status: 409, body: { title: "different body" } }]);
    const session = createChatSession({
      threadId: T,
      api: { getThread: vi.fn(), listMessages: vi.fn(), postMessage: (t, body) => postMessage(t, body) },
    });
    await expect(session.send({ client_message_id: "c8", text: "x" })).rejects.toMatchObject({ kind: "conflict" });
    expect(seen).toHaveLength(1);
    expect(session.getSnapshot().chat.pending.c8?.state).toBe("failed");
    // 人が再試行しても同じ吹き出しのまま。
    jsonFetch([{ status: 202, body: { message: message("m8", 8, { client_message_id: "c8" }), queue_position: 1 } }]);
    await session.send({ client_message_id: "c8", text: "x" });
    expect(Object.keys(session.getSnapshot().chat.pending)).toEqual([]);
    expect(Object.keys(session.getSnapshot().chat.messages)).toEqual(["m8"]);
  });

  it("chat_data_send_with_retry_gives_up_after_attempts", async () => {
    vi.useFakeTimers();
    const send = vi.fn(async () => {
      throw new ApiError("network", { method: "POST", path: "/api/v1/x" });
    });
    const result = sendWithRetry(send, { attempts: 2, delayMs: 10 });
    const settled = expect(result).rejects.toMatchObject({ kind: "network" });
    await vi.advanceTimersByTimeAsync(10);
    await settled;
    expect(send).toHaveBeenCalledTimes(2);
  });
});
