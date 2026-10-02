import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonKeys } from "../queries/keys";
import { createConnectionStore } from "./connection-state";
import { FakeEventSource, taskEventRow } from "./fake-event-source";
import { createRealtime } from "./realtime";
import { bindResume } from "./resume";
import { backoffDelay } from "./transport";

const flush = async () => {
  await vi.advanceTimersByTimeAsync(0);
};

function setup(probe: () => Promise<"ok" | "unauthorized"> = async () => "ok") {
  const queryClient = new QueryClient();
  const store = createConnectionStore();
  const taskEvents: number[] = [];
  const resets: number[] = [];
  const rt = createRealtime({
    queryClient,
    store,
    createEventSource: FakeEventSource.create,
    probe,
    onTaskEvent: (f) => taskEvents.push(f.data.id),
    onReset: (f) => resets.push(f.data.cursor),
  });
  return { queryClient, store, rt, taskEvents, resets };
}

beforeEach(() => {
  vi.useFakeTimers();
  FakeEventSource.reset();
});
afterEach(() => vi.useRealTimers());

describe("transport", () => {
  it("connects once and reports open", () => {
    const { rt, store } = setup();
    rt.start();
    expect(store.get()).toBe("connecting");
    FakeEventSource.last().open();
    expect(store.get()).toBe("open");
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(FakeEventSource.last().url).toBe("/events");
    rt.dispose();
    expect(store.get()).toBe("closed");
  });

  it("hello and heartbeat never reach the invalidation callbacks", () => {
    const { rt, taskEvents, resets, queryClient } = setup();
    const spy = vi.spyOn(queryClient, "invalidateQueries");
    rt.start();
    const es = FakeEventSource.last();
    es.emit("hello", { cursor: 5, now: "2026-09-30T00:00:00Z", daemon: null });
    es.emit("heartbeat", {});
    expect(taskEvents).toEqual([]);
    expect(resets).toEqual([]);
    expect(spy).not.toHaveBeenCalled();
    expect(rt.cursor()).toBe(5);
  });

  it("daemon frames update only ['daemon','stream']", () => {
    const { rt, queryClient } = setup();
    queryClient.setQueryData(daemonKeys.rest(), { rest: true });
    rt.start();
    const es = FakeEventSource.last();
    es.emit("daemon", { instances: [], tick: 1 });
    es.emit("hello", { cursor: 0, now: "x", daemon: { tick: 2 } });
    expect(queryClient.getQueryData(daemonKeys.stream())).toEqual({ tick: 2 });
    expect(queryClient.getQueryData(daemonKeys.rest())).toEqual({ rest: true });
  });

  it("drops invalid payloads and unknown event kinds", () => {
    const { rt, taskEvents, queryClient } = setup();
    rt.start();
    const es = FakeEventSource.last();
    es.emit("task.event", "not json");
    es.emit("task.event", { id: "1" });
    es.emit("task.event", taskEventRow(1, "no_such_kind"));
    es.emit("task.event", { ...taskEventRow(2, "created"), task_id: 3 });
    es.emit("daemon", "[1]");
    es.emit("reset", { cursor: "x" });
    es.emit("bogus", {});
    expect(taskEvents).toEqual([]);
    expect(queryClient.getQueryData(daemonKeys.stream())).toBeUndefined();
    expect(rt.cursor()).toBeUndefined();
  });

  it("tracks the cursor, drops duplicates and reversed ids, and adopts reset cursor", () => {
    const { rt, taskEvents, resets } = setup();
    rt.start();
    const es = FakeEventSource.last();
    es.emit("task.event", taskEventRow(3, "created"));
    es.emit("task.event", taskEventRow(3, "created"));
    es.emit("task.event", taskEventRow(2, "transitioned"));
    es.emit("task.event", taskEventRow(4, "transitioned"));
    expect(taskEvents).toEqual([3, 4]);
    es.emit("reset", { reason: "cursor_ahead", cursor: 1 });
    expect(resets).toEqual([1]);
    expect(rt.cursor()).toBe(1);
  });

  it("reconnects with after_id and capped exponential backoff", async () => {
    const { rt, store } = setup();
    rt.start();
    FakeEventSource.last().open();
    FakeEventSource.last().emit("task.event", taskEventRow(7, "created"));
    FakeEventSource.last().fail();
    await flush();
    expect(store.get()).toBe("reconnecting");
    expect(FakeEventSource.instances).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(999);
    expect(FakeEventSource.instances).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(FakeEventSource.instances).toHaveLength(2);
    expect(FakeEventSource.last().url).toBe("/events?after_id=7");
    FakeEventSource.last().fail(); // 開けないまま失敗 → 2 s
    await flush();
    await vi.advanceTimersByTimeAsync(1_999);
    expect(FakeEventSource.instances).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(FakeEventSource.instances).toHaveLength(3);
    expect(FakeEventSource.instances[0]?.closed).toBe(true);
    expect(backoffDelay(10)).toBe(30_000);
  });

  it("stops reconnecting on 401", async () => {
    const { rt, store } = setup(async () => "unauthorized");
    rt.start();
    FakeEventSource.last().open();
    FakeEventSource.last().fail();
    await flush();
    expect(store.get()).toBe("unauthorized");
    await vi.advanceTimersByTimeAsync(120_000);
    expect(FakeEventSource.instances).toHaveLength(1);
    rt.resume();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(FakeEventSource.instances).toHaveLength(1);
  });

  it("resume reconnects immediately without waiting for backoff", async () => {
    const { rt } = setup();
    rt.start();
    FakeEventSource.last().open();
    FakeEventSource.last().fail();
    await flush();
    rt.resume();
    expect(FakeEventSource.instances).toHaveLength(2);
  });
});

describe("resume trigger", () => {
  it("bundles visibilitychange/pageshow/online/focus into one debounced resume", () => {
    const handlers = new Map<string, () => void>();
    const target = {
      visibilityState: "visible",
      addEventListener: (t: string, l: () => void) => void handlers.set(t, l),
      removeEventListener: (t: string) => void handlers.delete(t),
    };
    const onResume = vi.fn();
    const unbind = bindResume({ onResume, debounceMs: 300, windowTarget: target, documentTarget: target });
    for (const t of ["visibilitychange", "pageshow", "online", "focus"]) handlers.get(t)?.();
    vi.advanceTimersByTime(299);
    expect(onResume).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onResume).toHaveBeenCalledTimes(1);
    target.visibilityState = "hidden";
    handlers.get("visibilitychange")?.();
    vi.advanceTimersByTime(1_000);
    expect(onResume).toHaveBeenCalledTimes(1);
    unbind();
    expect(handlers.size).toBe(0);
  });
});
