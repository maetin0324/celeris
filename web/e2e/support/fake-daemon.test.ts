import { afterEach, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import { createFakeDaemon, defaultFixtures, richFixtures, validateFixture } from "./fake-daemon.mjs";

const props = schema.properties as Record<string, unknown>;

let daemon: ReturnType<typeof createFakeDaemon> | undefined;
afterEach(async () => {
  if (daemon) await daemon.close();
  daemon = undefined;
});

it("provides responses that conform to the committed API schema", () => {
  for (const name of ["health", "inbox", "daemon"] as const) {
    expect(validateFixture(defaultFixtures[`/api/v1/${name}`], schema.properties[name])).toEqual([]);
  }
});

it("serves schema-valid rich data without changing the default profile", async () => {
  const rich = richFixtures();
  for (const [path, name] of [
    ["tasks", "task_list"],
    ["graph", "graph"],
    ["tasks/T1", "task_detail"],
    ["tasks/T1/changes", "changes"],
    ["tasks/T1/tree", "tree"],
    ["tasks/T1/artifacts", "artifact_list"],
    ["projects/P1", "project_detail"],
    ["console", "console"],
  ] as const) {
    const raw = rich[`/api/v1/${path}`];
    const value = typeof raw === "function" ? raw(new URL(`http://fixture.test/${path}`)) : raw;
    expect(validateFixture(value, props[name]), path).toEqual([]);
  }
  daemon = createFakeDaemon({ profile: "rich" });
  const url = await daemon.start();
  const get = async (path: string) => {
    const response = await fetch(`${url}/api/v1${path}`);
    expect(response.status, path).toBe(200);
    return response;
  };
  expect(((await (await get("/tasks")).json()) as { items: unknown[] }).items).toHaveLength(24);
  expect(((await (await get("/graph")).json()) as { nodes: unknown[] }).nodes).toHaveLength(8);
  expect(
    ((await (await get("/tasks/T1/changes")).json()) as { repos: Array<{ files: unknown[] }> }).repos[0].files,
  ).toHaveLength(15);
  expect(((await (await get("/tasks/T1/artifacts")).json()) as { items: unknown[] }).items).toHaveLength(12);
  expect((await (await get("/tasks/T1/runs/R1/stdout.jsonl")).text()).split("\n")).toHaveLength(28);
  expect(((await (await get("/console")).json()) as { items: unknown[] }).items).toHaveLength(3);
  expect(defaultFixtures["/api/v1/console"]).not.toEqual(rich["/api/v1/console"]);
});

it("serves schema-valid reports, approvals and review-pending execution data in the rich profile", () => {
  const rich = richFixtures();
  const checks = [
    ["/api/v1/reports", "report_list"],
    ["/api/v1/reports/RP1", "report_detail"],
    ["/api/v1/approvals", "approval_list"],
    ["/api/v1/tasks/T1/execution", "task_execution"],
    ["/api/v1/tasks/T1/routing", "task_routing"],
  ] as const;
  for (const [path, name] of checks) expect(validateFixture(rich[path], props[name]), path).toEqual([]);
});

it("rejects non-loopback and reserved production or staging ports", () => {
  expect(() => createFakeDaemon({ host: "0.0.0.0" })).toThrow("loopback");
  for (const port of [7700, 7701, 7710, 7711, 7712]) expect(() => createFakeDaemon({ port })).toThrow("reserved port");
  expect(() => createFakeDaemon({ delayMs: 100 })).toThrow("JSON delay");
});

it("records JSON requests, sends SSE immediately, and marks aborted requests", async () => {
  daemon = createFakeDaemon();
  const url = await daemon.start();
  const health = await fetch(`${url}/health`);
  expect(health.status).toBe(200);
  expect(await health.json()).toEqual(defaultFixtures["/health"]);
  daemon.setDelay(5000);
  const stream = await fetch(`${url}/events`);
  expect(stream.headers.get("content-type")).toContain("text/event-stream");
  if (!stream.body) throw new Error("SSE body missing");
  const reader = stream.body.getReader();
  await reader.read();
  daemon.sendEvent("task.event", { task_id: "t1" });
  const frame = new TextDecoder().decode((await reader.read()).value);
  expect(frame).toContain("event: task.event");
  expect(frame).toContain("t1");
  const tick = new TextDecoder().decode((await reader.read()).value);
  expect(tick).toContain("event: daemon");
  const snapshot = JSON.parse(tick.match(/^data: (.+)$/m)?.[1] ?? "null");
  expect(validateFixture(snapshot, schema.$defs.DaemonSnapshot)).toEqual([]);
  await reader.cancel();
  const controller = new AbortController();
  const pending = fetch(`${url}/inbox`, { signal: controller.signal });
  await new Promise((resolve) => setTimeout(resolve, 30));
  controller.abort();
  await expect(pending).rejects.toThrow();
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(
    daemon.requests.some((request: { path: string; aborted: boolean }) => request.path === "/inbox" && request.aborted),
  ).toBe(true);
});

// ADR-0133: 受信箱・通知の fixture は状態を持ち、answer・read・read-all で変わり、SSE の合図を送る。
it("serves stateful inbox items and notifications that match the schema", async () => {
  daemon = createFakeDaemon();
  const url = await daemon.start();
  const stream = await fetch(`${url}/api/v1/stream`);
  if (!stream.body) throw new Error("SSE body missing");
  const reader = stream.body.getReader();
  const decoder = new TextDecoder();
  let seen = "";
  const waitFor = async (name: string) => {
    while (!seen.includes(`event: ${name}`)) seen += decoder.decode((await reader.read()).value);
    seen = "";
  };
  const get = async (path: string) => (await fetch(`${url}/api/v1${path}`)).json();
  const post = (path: string, body?: unknown) =>
    fetch(`${url}/api/v1${path}`, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });

  const inbox = await get("/inbox/items");
  expect(validateFixture(inbox, props.inbox_items)).toEqual([]);
  const kinds = inbox.items.map((item: { kind: string }) => item.kind);
  expect(kinds).toEqual(["decision", "plan_gate", "failed", "authorization", "knowledge_review"]);
  expect(inbox.counts.total).toBe(5);
  expect((await get("/inbox/items?kind=decision")).counts).toEqual({ total: 1, by_kind: { decision: 1 } });
  const one = await get("/inbox/items/decision%3AD1");
  expect(validateFixture(one, props.inbox_item)).toEqual([]);

  expect((await post("/inbox/items/decision%3AD1/answer", { option: "nope" })).status).toBe(422);
  expect((await post("/inbox/items/failed%3AT3/answer", { option: "cancel" })).status).toBe(422);
  const answered = await post("/inbox/items/decision%3AD1/answer", { option: "session", note: "ok" });
  const result = await answered.json();
  expect(validateFixture(result, props.inbox_answer_result)).toEqual([]);
  expect(result).toMatchObject({ item_id: "decision:D1", removed: true });
  await waitFor("inbox_changed");
  expect((await get("/inbox/items")).counts.total).toBe(4);
  expect((await fetch(`${url}/api/v1/inbox/items/decision%3AD1`)).status).toBe(404);
  expect(daemon.inbox.answers).toEqual([{ id: "decision:D1", option: "session", note: "ok" }]);

  const notices = await get("/notifications");
  expect(validateFixture(notices, props.notifications)).toEqual([]);
  expect(notices.unread).toBe(3);
  expect(notices.items.map((n: { id: string }) => n.id)).toEqual(["N1", "N2", "N3", "N4"]);
  const page = await get("/notifications?limit=2");
  expect(page.next_before).toBe("2026-10-03T09:00:00Z");
  expect((await get(`/notifications?before=${encodeURIComponent(page.next_before)}`)).items[0].id).toBe("N3");
  const count = await get("/notifications/unread-count");
  expect(validateFixture(count, props.notifications_unread_count)).toEqual([]);
  expect(count).toEqual({ unread: 3, events: 6, by_kind: { task_done: 1, report: 1, bad_news: 1 } });

  const read = await (await post("/notifications/N1/read")).json();
  expect(validateFixture(read, props.notification_read)).toEqual([]);
  await waitFor("notifications_changed");
  expect((await post("/notifications/N1/read")).status).toBe(200);
  expect((await get("/notifications?unread=true")).items.map((n: { id: string }) => n.id)).toEqual(["N2", "N3"]);
  const all = await (await post("/notifications/read-all", { kind: "report" })).json();
  expect(validateFixture(all, props.notifications_read_all)).toEqual([]);
  expect(all).toEqual({ marked: 1 });
  await waitFor("notifications_changed");
  expect(await (await post("/notifications/read-all", {})).json()).toEqual({ marked: 1 });
  expect((await get("/notifications/unread-count")).unread).toBe(0);
  await reader.cancel();
});
