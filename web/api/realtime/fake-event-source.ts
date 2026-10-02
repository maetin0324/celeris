// テスト用の EventSource。外部ネットワークには出ない。

import type { EventSourceLike, MessageLike } from "./transport";

export class FakeEventSource implements EventSourceLike {
  static instances: FakeEventSource[] = [];
  static create = (url: string): FakeEventSource => new FakeEventSource(url);
  static reset(): void {
    FakeEventSource.instances = [];
  }
  static last(): FakeEventSource {
    const last = FakeEventSource.instances.at(-1);
    if (!last) throw new Error("no EventSource created");
    return last;
  }

  closed = false;
  onopen: ((event: unknown) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  private listeners = new Map<string, Array<(event: MessageLike) => void>>();

  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  addEventListener(type: string, listener: (event: MessageLike) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  close(): void {
    this.closed = true;
  }
  open(): void {
    this.onopen?.({});
  }
  fail(): void {
    this.onerror?.({});
  }
  emit(type: string, data: unknown): void {
    const text = typeof data === "string" ? data : JSON.stringify(data);
    for (const l of this.listeners.get(type) ?? []) l({ data: text });
  }
}

export function taskEventRow(id: number, kind: string, taskId = "T1", extra: Record<string, unknown> = {}) {
  return { id, seq: id, task_id: taskId, ts: "2026-09-30T00:00:00Z", event: { type: kind, ...extra } };
}
