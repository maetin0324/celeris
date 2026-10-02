// SSE の transport（ADR-0081 D6）。shell で 1 本だけ購読する。
// - 再接続は自前で行う（EventSource の自動再接続は 401 を区別できない）。メモリ上の cursor を `after_id` に載せる。
// - 失敗時は上限付き指数 backoff。401 は probe で確かめ、再接続を止める（state = unauthorized）。
// - cursor 以下の id の task.event（重複・逆順）は捨てる。

import { ApiError, apiFetch } from "../client";
import type { ConnectionStore } from "./connection-state";
import { FRAME_TYPES, type Frame, parseFrame } from "./frames";

export type MessageLike = { data: unknown };

export type EventSourceLike = {
  close(): void;
  addEventListener(type: string, listener: (event: MessageLike) => void): void;
  onopen: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
};

export type EventSourceFactory = (url: string) => EventSourceLike;

export type ProbeResult = "ok" | "unauthorized";

export type TransportOptions = {
  store: ConnectionStore;
  onFrame: (frame: Frame) => void;
  /** resume の契機ごとに呼ぶ（接続中でも）。active な stale query の再取得はここで行う。 */
  onResume?: () => void;
  url?: string;
  createEventSource?: EventSourceFactory;
  /** 切断の原因が 401 かを確かめる。 */
  probe?: () => Promise<ProbeResult>;
  backoffBaseMs?: number;
  backoffMaxMs?: number;
};

export const BACKOFF_BASE_MS = 1_000;
export const BACKOFF_MAX_MS = 30_000;

export function backoffDelay(attempt: number, base = BACKOFF_BASE_MS, max = BACKOFF_MAX_MS): number {
  return Math.min(base * 2 ** attempt, max);
}

export async function defaultProbe(): Promise<ProbeResult> {
  try {
    await apiFetch("/api/v1/health");
    return "ok";
  } catch (error) {
    if (error instanceof ApiError && error.kind === "unauthorized") return "unauthorized";
    return "ok";
  }
}

const defaultFactory: EventSourceFactory = (url) => new EventSource(url) as unknown as EventSourceLike;

export type Transport = {
  start(): void;
  stop(): void;
  /** 復帰。接続が生きていなければ backoff を待たず張り直す。 */
  resume(): void;
  cursor(): number | undefined;
};

export function createTransport(options: TransportOptions): Transport {
  const base = options.url ?? "/events";
  const factory = options.createEventSource ?? defaultFactory;
  const probe = options.probe ?? defaultProbe;
  const store = options.store;
  let source: EventSourceLike | undefined;
  let cursor: number | undefined;
  let attempt = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let everOpened = false;
  let generation = 0;

  function closeSource() {
    const s = source;
    source = undefined;
    if (s) {
      s.onopen = null;
      s.onerror = null;
      s.close();
    }
  }

  function connect() {
    if (!running) return;
    timer = undefined;
    closeSource();
    const gen = ++generation;
    store.set(everOpened ? "reconnecting" : "connecting");
    const url = cursor === undefined ? base : `${base}?after_id=${cursor}`;
    const es = factory(url);
    source = es;
    es.onopen = () => {
      if (gen !== generation) return;
      attempt = 0;
      everOpened = true;
      store.set("open");
    };
    for (const name of FRAME_TYPES) {
      es.addEventListener(name, (message) => {
        if (gen !== generation) return;
        handle(name, message.data);
      });
    }
    es.onerror = () => {
      if (gen !== generation) return;
      void onDisconnected();
    };
  }

  function handle(name: string, data: unknown) {
    if (typeof data !== "string") return;
    const frame = parseFrame(name, data);
    if (!frame) return; // 不正な payload は捨てる
    if (frame.type === "task.event") {
      if (cursor !== undefined && frame.data.id <= cursor) return; // 重複・逆順
      cursor = frame.data.id;
    } else if (frame.type === "hello") {
      if (cursor === undefined) cursor = frame.data.cursor;
    } else if (frame.type === "reset") {
      cursor = frame.data.cursor;
    }
    options.onFrame(frame);
  }

  async function onDisconnected() {
    closeSource();
    generation++;
    if (!running) return;
    store.set("reconnecting");
    const gen = generation;
    const result = await probe();
    if (!running || gen !== generation) return;
    if (result === "unauthorized") {
      running = false;
      store.set("unauthorized");
      return;
    }
    const delay = backoffDelay(attempt++, options.backoffBaseMs, options.backoffMaxMs);
    timer = setTimeout(connect, delay);
  }

  return {
    start() {
      if (running) return;
      running = true;
      attempt = 0;
      connect();
    },
    stop() {
      running = false;
      generation++;
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      closeSource();
      if (store.get() !== "unauthorized") store.set("closed");
    },
    resume() {
      if (!running) return;
      options.onResume?.();
      if (store.get() === "open") return;
      if (timer !== undefined) {
        clearTimeout(timer);
        timer = undefined;
        attempt = 0;
        connect();
      }
    },
    cursor: () => cursor,
  };
}
