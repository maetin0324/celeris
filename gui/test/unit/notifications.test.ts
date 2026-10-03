import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { loadNotifications, markNoticeRead } from "~/routes/notifications";
import { type MockCeleris, sendJson, startMockCeleris } from "../mock-celeris/server";

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => mock.close());

describe("通知画面 API", () => {
  it("通知一覧と未読数を取得する", async () => {
    mock.on("GET", "/api/v1/notifications", (_req, res) => sendJson(res, 200, {
      items: [{ id: "n1", kind: "task_done", group_key: "task_done:p1", title: "完了", summary: "タスクが完了", count: 2, first_at: "2026-10-01T00:00:00Z", last_at: "2026-10-02T00:00:00Z", read_at: null }],
      unread: 2,
    }));
    mock.on("GET", "/api/v1/notifications/unread-count", (_req, res) => sendJson(res, 200, { unread: 2, by_kind: { task_done: 2 } }));

    const result = await loadNotifications(client, new Request("http://gui.invalid/notifications"));

    expect(result.unread).toBe(2);
    expect(result.feed.items[0]?.id).toBe("n1");
  });

  it("通知を既読化する", async () => {
    mock.on("POST", "/api/v1/notifications/n1/read", (_req, res) => sendJson(res, 200, { id: "n1", read_at: "2026-10-02T00:00:00Z" }));

    await expect(markNoticeRead(client, "n1")).resolves.toBeUndefined();
  });
});
