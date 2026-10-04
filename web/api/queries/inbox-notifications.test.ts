import { hashKey } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApiClient } from "../client";
import type { HumanInboxView, UnreadCountView } from "../generated/types";
import { badgeView, inboxItemsBadge, notificationsUnreadBadge } from "./badges";
import {
  answerInboxItem,
  inboxAnswerInvalidates,
  inboxItemQuery,
  inboxItemsQuery,
  markAllNotificationsRead,
  markNotificationRead,
  notificationReadInvalidates,
  notificationsQuery,
  unreadCountQuery,
} from "./inbox-notifications";
import { inboxKeys, notificationKeys } from "./keys";

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });

type Call = { url: string; method: string; body: unknown };

function recordingFetcher(response: unknown) {
  const calls: Call[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    });
    return json(response);
  });
  configureApiClient({ fetcher: fetcher as unknown as typeof fetch });
  return calls;
}

const signal = () => new AbortController().signal;

afterEach(() => {
  configureApiClient({ fetcher: (i, init) => fetch(i, init), onUnauthorized: () => {} });
});

describe("受信箱の query（GET /inbox/items）", () => {
  it("key は ['inbox','items',filters]、URL は同じ正規化の query を持つ", async () => {
    const calls = recordingFetcher({ counts: { total: 0, by_kind: {} }, items: [], suppressed: {} });
    const q = inboxItemsQuery({ kind: "decision", project: " P1 " });
    expect(q.queryKey).toEqual(["inbox", "items", { kind: "decision", project: "P1" }]);
    await q.queryFn({ signal: signal() });
    expect(calls[0]).toEqual({ url: "/api/inbox/items?kind=decision&project=P1", method: "GET", body: undefined });
    await inboxItemsQuery().queryFn({ signal: signal() });
    expect(calls[1].url).toBe("/api/inbox/items");
  });

  it("同じ条件は同じ key、旧 GET /inbox の key とは別で、どちらも ['inbox'] の下", () => {
    expect(hashKey(inboxItemsQuery({ project: "P1" }).queryKey)).toBe(
      hashKey(inboxItemsQuery({ project: "P1 ", kind: undefined }).queryKey),
    );
    expect(hashKey(inboxKeys.itemList())).not.toBe(hashKey(inboxKeys.list()));
    for (const key of [inboxKeys.itemList(), inboxKeys.item("I1"), inboxKeys.list()]) expect(key[0]).toBe("inbox");
  });

  it("1 件の取得と回答は id を path に入れて送る", async () => {
    const calls = recordingFetcher({ item_id: "a/b", removed: true, result: null });
    expect(inboxItemQuery("a/b").queryKey).toEqual(["inbox", "item", "a/b"]);
    await inboxItemQuery("a/b").queryFn({ signal: signal() });
    const result = await answerInboxItem("a/b", { option: "approve", note: "ok" });
    expect(calls[0]).toMatchObject({ url: "/api/inbox/items/a%2Fb", method: "GET" });
    expect(calls[1]).toEqual({
      url: "/api/inbox/items/a%2Fb/answer",
      method: "POST",
      body: { option: "approve", note: "ok" },
    });
    expect(result.removed).toBe(true);
    expect(inboxAnswerInvalidates).toEqual([inboxKeys.all]);
  });
});

describe("通知の query（GET /notifications・unread-count）", () => {
  it("一覧は filter を key と URL に持つ", async () => {
    const calls = recordingFetcher({ items: [], unread: 0, next_before: null });
    const q = notificationsQuery({ unread: true, kind: "report", limit: 50 });
    expect(q.queryKey).toEqual(["notifications", "list", { kind: "report", limit: 50, unread: true }]);
    await q.queryFn({ signal: signal() });
    expect(calls[0].url).toBe("/api/notifications?kind=report&limit=50&unread=true");
  });

  it("未読数・既読化・全部既読の path と body", async () => {
    const calls = recordingFetcher({ unread: 2, events: 5, by_kind: {} });
    expect(unreadCountQuery.queryKey).toEqual(notificationKeys.unreadCount());
    await unreadCountQuery.queryFn({ signal: signal() });
    await markNotificationRead("N1");
    await markAllNotificationsRead({ kind: "report" });
    await markAllNotificationsRead();
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ["GET", "/api/notifications/unread-count", undefined],
      ["POST", "/api/notifications/N1/read", undefined],
      ["POST", "/api/notifications/read-all", { kind: "report" }],
      ["POST", "/api/notifications/read-all", {}],
    ]);
    expect(notificationReadInvalidates).toEqual([notificationKeys.all]);
  });
});

describe("件数バッジ", () => {
  it("受信箱は counts.total、通知は未読の束数（events ではない）", () => {
    const inbox: HumanInboxView = {
      counts: { total: 3, by_kind: { decision: 2, failed: 1 } },
      items: [],
      suppressed: {},
    };
    const unread: UnreadCountView = { unread: 2, events: 9, by_kind: { report: 2 } };
    expect(inboxItemsBadge.select(inbox)).toBe(3);
    expect(notificationsUnreadBadge.select(unread)).toBe(2);
    expect(badgeView(inboxItemsBadge.select(inbox), "受信箱")).toEqual({ text: "3", label: "受信箱 3 件" });
    expect(badgeView(notificationsUnreadBadge.select(unread), "未読の通知")).toEqual({
      text: "2",
      label: "未読の通知 2 件",
    });
  });
});
