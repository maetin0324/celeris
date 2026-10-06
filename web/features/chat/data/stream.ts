// チャット stream `GET /chat/threads/{t}/stream` の接続（ADR 2026-10-05-cos-chat-home D2）。
// EventSource は HTTP status を見せない（410 の cursor 失効を区別できない）ので、fetch の body を SSE として読む。
// - 再開は `Last-Event-ID` ヘッダと `?after=` に同じ cursor を載せる（不一致は 400 になるので必ず同じ値）。
// - cursor は呼び出し側（reducer の lastEventId）から接続のたびに読む。
// - 410 は onExpired（thread と messages を取り直す）。401 は止める。他の失敗は上限付き指数 backoff で張り直す。
// - 切断は run の停止とみなさない（ここでは run の状態に触れない）。

import { assertApiPath } from "../../../api/client";
import type { ChatEvent } from "../../../api/generated/types";
import { chatPaths } from "./client";

export type SseFrame = { event: string; id: string | undefined; data: string };

/** SSE の行を組み立てる。コメント（`: heartbeat`）は捨てる。chunk の境界は任意。 */
export class SseParser {
  private buffer = "";
  private event = "";
  private id: string | undefined;
  private data: string[] = [];

  push(chunk: string): SseFrame[] {
    this.buffer += chunk;
    const frames: SseFrame[] = [];
    for (;;) {
      const match = /\r\n|\r|\n/.exec(this.buffer);
      if (!match) break;
      // 末尾の \r だけでは \r\n の途中かもしれない。
      if (match[0] === "\r" && match.index === this.buffer.length - 1) break;
      const line = this.buffer.slice(0, match.index);
      this.buffer = this.buffer.slice(match.index + match[0].length);
      const frame = this.line(line);
      if (frame) frames.push(frame);
    }
    return frames;
  }

  private line(line: string): SseFrame | undefined {
    if (line === "") {
      const frame =
        this.data.length > 0 ? { event: this.event || "message", id: this.id, data: this.data.join("\n") } : undefined;
      this.event = "";
      this.id = undefined;
      this.data = [];
      return frame;
    }
    if (line.startsWith(":")) return undefined;
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") this.event = value;
    else if (field === "data") this.data.push(value);
    else if (field === "id" && !value.includes("\0")) this.id = value;
    return undefined;
  }
}

export const CHAT_EVENT_TYPES = ["message", "text_delta", "status", "tool", "run", "queue", "card", "thread"] as const;

/** frame を ChatEvent にする。知らない type・読めない JSON は undefined。 */
export function frameToEvent(frame: SseFrame): ChatEvent | undefined {
  if (!(CHAT_EVENT_TYPES as readonly string[]).includes(frame.event)) return undefined;
  try {
    const event = JSON.parse(frame.data) as ChatEvent;
    if (typeof event?.id !== "string" || event.type !== frame.event) return undefined;
    return event;
  } catch {
    return undefined;
  }
}

export type StreamState = "idle" | "connecting" | "open" | "reconnecting" | "expired" | "unauthorized" | "closed";

export type ChatStreamOptions = {
  threadId: string;
  /** 接続のたびに読む cursor（適用済みの最後の id）。 */
  cursor: () => string;
  onEvent: (event: ChatEvent) => void;
  /** 410（cursor 失効）。呼び出し側が取り直してから start() し直す。 */
  onExpired: () => void;
  onState?: (state: StreamState) => void;
  fetcher?: typeof fetch;
  backoffBaseMs?: number;
  backoffMaxMs?: number;
};

export type ChatStream = {
  start(): void;
  stop(): void;
  /** 張り直し（復帰時など）。backoff を待たない。 */
  reconnect(): void;
  state(): StreamState;
};

export const STREAM_BACKOFF_BASE_MS = 1_000;
export const STREAM_BACKOFF_MAX_MS = 30_000;

export function streamUrl(threadId: string, cursor: string): string {
  return `${chatPaths.stream(threadId)}?after=${encodeURIComponent(cursor)}`;
}

export function createChatStream(options: ChatStreamOptions): ChatStream {
  const fetcher = options.fetcher ?? ((input, init) => fetch(input, init));
  const base = options.backoffBaseMs ?? STREAM_BACKOFF_BASE_MS;
  const max = options.backoffMaxMs ?? STREAM_BACKOFF_MAX_MS;
  let state: StreamState = "idle";
  let controller: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let attempt = 0;
  let generation = 0;

  function setState(next: StreamState) {
    if (state === next) return;
    state = next;
    options.onState?.(next);
  }

  function clearTimer() {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  }

  function abort() {
    controller?.abort(new DOMException("stream closed", "AbortError"));
    controller = undefined;
  }

  function scheduleReconnect(gen: number) {
    if (gen !== generation) return;
    setState("reconnecting");
    const delay = Math.min(base * 2 ** attempt, max);
    attempt += 1;
    clearTimer();
    timer = setTimeout(() => {
      timer = undefined;
      if (gen === generation) void connect(gen);
    }, delay);
  }

  async function connect(gen: number) {
    abort();
    const own = new AbortController();
    controller = own;
    const cursor = options.cursor();
    const url = streamUrl(options.threadId, cursor);
    assertApiPath(url);
    if (state !== "reconnecting") setState("connecting");
    let response: Response;
    try {
      response = await fetcher(url, {
        method: "GET",
        headers: { Accept: "text/event-stream", "Last-Event-ID": cursor },
        credentials: "same-origin",
        cache: "no-store",
        signal: own.signal,
      });
    } catch {
      if (gen === generation && !own.signal.aborted) scheduleReconnect(gen);
      return;
    }
    if (gen !== generation) return;
    if (response.status === 410) {
      setState("expired");
      options.onExpired();
      return;
    }
    if (response.status === 401) {
      setState("unauthorized");
      return;
    }
    if (!response.ok || !response.body) {
      scheduleReconnect(gen);
      return;
    }
    attempt = 0;
    setState("open");
    const parser = new SseParser();
    const decoder = new TextDecoder();
    const reader = response.body.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (gen !== generation) return;
        if (done) break;
        for (const frame of parser.push(decoder.decode(value, { stream: true }))) {
          const event = frameToEvent(frame);
          if (event) options.onEvent(event);
          if (gen !== generation) return;
        }
      }
    } catch {
      // 読み取り中の切断。下で張り直す。
    }
    if (gen === generation && !own.signal.aborted) scheduleReconnect(gen);
  }

  return {
    start() {
      generation += 1;
      clearTimer();
      attempt = 0;
      setState("connecting");
      void connect(generation);
    },
    stop() {
      generation += 1;
      clearTimer();
      abort();
      setState("closed");
    },
    reconnect() {
      if (state === "open" || state === "connecting") return;
      if (state === "unauthorized") return;
      generation += 1;
      clearTimer();
      setState("connecting");
      void connect(generation);
    },
    state: () => state,
  };
}
