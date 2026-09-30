import type { ConsoleBlock, ConsoleHello } from "../../api/generated/types";

// `/console/stream` の SSE client（P3-01）。`/events` の transport（api/realtime）とは別の接続で、
// 再取得ではなく block の差分を渡す。切断の後は最後に受けた cursor を `since` にして張り直す。

export type ConsoleStreamHandlers = {
  onHello?: (hello: ConsoleHello) => void;
  onBlock: (block: ConsoleBlock) => void;
  onOpen?: () => void;
  onError?: () => void;
};

export type EventSourceLike = {
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void;
  close(): void;
  onerror: ((event: Event) => void) | null;
  onopen: ((event: Event) => void) | null;
};

export type ConsoleStreamOptions = ConsoleStreamHandlers & {
  scope: string;
  since?: string | null;
  retryMs?: number;
  createSource?: (url: string) => EventSourceLike;
};

export function consoleStreamUrl(scope: string, since: string | null): string {
  const params = new URLSearchParams({ scope });
  if (since) params.set("since", since);
  return `/console/stream?${params}`;
}

function parse<T>(raw: string): T | null {
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null; // 壊れた frame は捨て、以後の frame を受け続ける
  }
}

/** 接続を張り、止める関数を返す。cursor() は次に張り直すときの since。 */
export function openConsoleStream(options: ConsoleStreamOptions): { close: () => void; cursor: () => string | null } {
  const { scope, retryMs = 1_000 } = options;
  const create = options.createSource ?? ((url) => new EventSource(url) as unknown as EventSourceLike);
  let cursor = options.since ?? null;
  let closed = false;
  let source: EventSourceLike | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function connect() {
    if (closed) return;
    const es = create(consoleStreamUrl(scope, cursor));
    source = es;
    es.onopen = () => options.onOpen?.();
    es.addEventListener("hello", (event) => {
      const hello = parse<ConsoleHello>(event.data);
      if (!hello) return;
      cursor = hello.cursor;
      options.onHello?.(hello);
    });
    es.addEventListener("console.block", (event) => {
      const block = parse<ConsoleBlock>(event.data);
      if (!block) return;
      cursor = block.cursor;
      options.onBlock(block);
    });
    es.onerror = () => {
      if (closed) return;
      es.close(); // 既定の再接続は最初の since のままなので使わない
      options.onError?.();
      timer = setTimeout(connect, retryMs);
    };
  }
  connect();
  return {
    cursor: () => cursor,
    close() {
      closed = true;
      if (timer !== null) clearTimeout(timer);
      source?.close();
    },
  };
}
