import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { loadInbox } from "~/routes/inbox";
import { loadNotifications, markNoticeRead } from "~/routes/notifications";
import { sendJson, startMockCeleris, type MockCeleris } from "../mock-celeris/server";

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => {
  await mock.close();
});

describe("legacy inbox and notification routes", () => {
  it("keeps loading the inbox from the legacy /inbox endpoint", async () => {
    const inbox = {
      approvals: [],
      questions: [],
      drafts: [],
      attention: [],
      browser_waits: [],
      decisions: [],
      counts: { approvals: 0, questions: 0, drafts: 0, attention: 0, browser_waits: 0, decisions: 0, by_status: {} },
    };
    mock.on("GET", "/api/v1/inbox", (_req, res) => sendJson(res, 200, inbox));

    await expect(loadInbox(client, new Request("http://gui.invalid/"))).resolves.toMatchObject(inbox);
    expect(mock.requests.map(({ method, url }) => [method, url])).toEqual([["GET", "/api/v1/inbox"]]);
  });

  it("loads the notice feed and unread count, then marks a notice read", async () => {
    const feed = {
      items: [{ id: "n/1", kind: "task_done", title: "完了", summary: "報告", count: 2, last_at: "2026-10-02T00:00:00Z" }],
      unread: 1,
    };
    mock.on("GET", "/api/v1/notifications", (_req, res) => sendJson(res, 200, feed));
    mock.on("GET", "/api/v1/notifications/unread-count", (_req, res) => sendJson(res, 200, { unread: 1 }));
    mock.on("POST", "/api/v1/notifications/n%2F1/read", (_req, res) => sendJson(res, 200, { ok: true }));

    await expect(loadNotifications(client, new Request("http://gui.invalid/notifications"))).resolves.toEqual({
      feed,
      unread: 1,
    });
    await markNoticeRead(client, "n/1");
    expect(mock.requests.map(({ method, url }) => [method, url])).toEqual([
      ["GET", "/api/v1/notifications"],
      ["GET", "/api/v1/notifications/unread-count"],
      ["POST", "/api/v1/notifications/n%2F1/read"],
    ]);
  });
});
