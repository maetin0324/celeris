// SSE の軽い合図 inbox_changed・notifications_changed（ADR-0133 D5）が受信箱・通知の key だけを取り直すこと。

import { QueryClient, type QueryKey, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonKeys, inboxKeys, notificationKeys, taskKeys } from "../queries/keys";
import { createConnectionStore } from "./connection-state";
import { FakeEventSource } from "./fake-event-source";
import { FRAME_TYPES, parseFrame } from "./frames";
import { SIGNAL_INVALIDATION } from "./invalidation-map";
import { createRealtime } from "./realtime";

describe("合図の frame", () => {
  it("購読する event 名に含まれ、data の中身によらず合図になる", () => {
    expect(FRAME_TYPES).toContain("inbox_changed");
    expect(FRAME_TYPES).toContain("notifications_changed");
    expect(parseFrame("inbox_changed", "{}")).toEqual({ type: "inbox_changed" });
    expect(parseFrame("notifications_changed", '{"extra":1}')).toEqual({ type: "notifications_changed" });
    expect(parseFrame("notifications_changed", "")).toEqual({ type: "notifications_changed" });
  });

  it("受信箱は ['inbox'] 全体、通知は ['notifications'] 全体", () => {
    expect(SIGNAL_INVALIDATION.inbox_changed).toEqual([inboxKeys.all]);
    expect(SIGNAL_INVALIDATION.notifications_changed).toEqual([notificationKeys.all]);
  });
});

describe("transport を通した invalidate", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup() {
    const qc = new QueryClient();
    const counts = new Map<string, number>();
    const watch = (name: string, queryKey: QueryKey) => {
      counts.set(name, 0);
      const observer = new QueryObserver(qc, {
        queryKey,
        staleTime: 60_000,
        queryFn: async () => {
          counts.set(name, (counts.get(name) ?? 0) + 1);
          return counts.get(name);
        },
      });
      return observer.subscribe(() => {});
    };
    const unsubs = [
      watch("items", inboxKeys.itemList()),
      watch("item", inboxKeys.item("I1")),
      watch("legacy", inboxKeys.list()),
      watch("notices", notificationKeys.list({ unread: true })),
      watch("unread", notificationKeys.unreadCount()),
      watch("tasks", taskKeys.lists()),
      watch("daemon", daemonKeys.rest()),
    ];
    FakeEventSource.reset();
    const rt = createRealtime({
      queryClient: qc,
      store: createConnectionStore(),
      createEventSource: FakeEventSource.create,
      probe: async () => "ok",
    });
    rt.start();
    const snapshot = () => Object.fromEntries(counts);
    return {
      es: FakeEventSource.last(),
      snapshot,
      dispose() {
        rt.dispose();
        for (const u of unsubs) u();
      },
    };
  }

  it("inbox_changed は受信箱の key だけを取り直し、連続した合図は 1 回に束ねる", async () => {
    const t = setup();
    await vi.advanceTimersByTimeAsync(10);
    const before = t.snapshot();
    for (let i = 0; i < 5; i++) t.es.emit("inbox_changed", {});
    await vi.advanceTimersByTimeAsync(1_000);
    const after = t.snapshot();
    expect(after.items - before.items).toBe(1);
    expect(after.item - before.item).toBe(1);
    expect(after.legacy - before.legacy).toBe(1);
    for (const name of ["notices", "unread", "tasks", "daemon"]) expect(after[name] - before[name], name).toBe(0);
    t.dispose();
  });

  it("notifications_changed は通知の一覧と未読数だけを取り直す", async () => {
    const t = setup();
    await vi.advanceTimersByTimeAsync(10);
    const before = t.snapshot();
    t.es.emit("notifications_changed", {});
    await vi.advanceTimersByTimeAsync(1_000);
    const after = t.snapshot();
    expect(after.notices - before.notices).toBe(1);
    expect(after.unread - before.unread).toBe(1);
    for (const name of ["items", "item", "legacy", "tasks", "daemon"]) expect(after[name] - before[name], name).toBe(0);
    t.dispose();
  });
});
