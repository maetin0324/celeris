// Node 用の最小 EventSource（parity e2e 用）。fetch で gateway の `/events` を読み、SSE を行単位で解く。
// 自動再接続はしない（再接続は transport の責務）。外部ネットワークには出ない。

import type { EventSourceLike, MessageLike } from "../../api/realtime/transport";

export class NodeEventSource implements EventSourceLike {
  static instances: NodeEventSource[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  status: number | undefined;
  private readonly controller = new AbortController();
  private readonly listeners = new Map<string, Array<(event: MessageLike) => void>>();
  private closed = false;

  constructor(readonly url: string) {
    NodeEventSource.instances.push(this);
    void this.run();
  }

  addEventListener(type: string, listener: (event: MessageLike) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
    this.controller.abort();
  }

  private fail() {
    if (this.closed) return;
    this.closed = true;
    this.onerror?.({});
  }

  private async run() {
    try {
      const res = await fetch(this.url, { headers: { accept: "text/event-stream" }, signal: this.controller.signal });
      this.status = res.status;
      if (res.status !== 200 || !res.body) {
        await res.body?.cancel();
        return this.fail();
      }
      this.onopen?.({});
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let event = "message";
      let data: string[] = [];
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let nl = buffer.indexOf("\n");
        while (nl >= 0) {
          const line = buffer.slice(0, nl).replace(/\r$/, "");
          buffer = buffer.slice(nl + 1);
          if (line === "") {
            if (data.length && !this.closed) {
              const message = { data: data.join("\n") };
              for (const l of this.listeners.get(event) ?? []) l(message);
            }
            event = "message";
            data = [];
          } else if (line.startsWith("event:")) event = line.slice(6).trim();
          else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
          nl = buffer.indexOf("\n");
        }
      }
    } catch {
      // abort・切断
    }
    this.fail();
  }
}
