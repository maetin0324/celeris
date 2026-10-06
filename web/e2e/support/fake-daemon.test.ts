import { generateKeyPairSync, sign } from "node:crypto";
import { afterEach, describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import {
  BROWSER_RAW_LIVE_VIEW_URL,
  createFakeDaemon,
  defaultFixtures,
  type FakeDaemonOptions,
  richFixtures,
  validateFixture,
} from "./fake-daemon.mjs";

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
    ["org", "org_list"],
    ["projects/P1/docs", "docs_tree"],
    ["projects/P1/docs/page", "doc_page"],
    ["tasks/T25", "task_detail"],
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
  expect(((await (await get("/tasks")).json()) as { items: unknown[] }).items).toHaveLength(25);
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

// 偽 browser backend（ADR 2026-10-05-browser-department-web-live-view D2.4）。
describe("browser backend", () => {
  const keys = generateKeyPairSync("ed25519");
  let clock = Date.parse("2026-10-04T09:00:00Z");
  let url = "";
  const assertion = (task: string, run: string, session: string, owner = "owner-a") => {
    const payload = JSON.stringify({
      task_id: task,
      run_id: run,
      browser_session_id: session,
      owner_session_id: owner,
      expires_at: Math.floor(clock / 1000) + 20,
    });
    return { payload, signature: sign(null, Buffer.from(payload), keys.privateKey).toString("hex") };
  };
  const call = async (method: string, path: string, body?: unknown) => {
    const response = await fetch(`${url}/api/v1${path}`, {
      method,
      headers: body ? { "content-type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
    const text = await response.text();
    return { status: response.status, body: text ? JSON.parse(text) : null };
  };
  const start = async (browser: FakeDaemonOptions["browser"] = {}) => {
    clock = Date.parse("2026-10-04T09:00:00Z");
    daemon = createFakeDaemon({
      browser: { publicKey: keys.publicKey, now: () => clock, ...(typeof browser === "object" ? browser : {}) },
    });
    url = await daemon.start();
  };

  it("is off by default and keeps the existing inbox", async () => {
    daemon = createFakeDaemon();
    url = await daemon.start();
    expect(daemon.browser).toBeNull();
    expect((await call("GET", "/tasks/T1/browser/waits")).status).toBe(404);
    const inbox = (await call("GET", "/inbox/items")).body;
    expect(inbox.items.some((item: { kind: string }) => item.kind === "browser_wait")).toBe(false);
  });

  it("serves T1/T2 browser history with raw live_view_url, task details and inbox items", async () => {
    await start();
    const events = (await call("GET", "/tasks/T1/events?types=browser_updated&after_seq=-1&limit=5000")).body;
    expect(validateFixture(events, props.events_page)).toEqual([]);
    expect(events.has_more).toBe(false);
    expect(events.items.map((row: { event: { browser: { run_id: string } } }) => row.event.browser.run_id)).toEqual([
      "R0",
      "R1",
      "R9",
      "R1",
    ]);
    expect(events.items[1].event.browser.live_view_url).toBe(BROWSER_RAW_LIVE_VIEW_URL);
    expect((await call("GET", "/tasks/T2/events?types=browser_updated")).body.items[0].event.browser).toMatchObject({
      task_id: "T2",
      run_id: "R2",
      session_id: "S2",
      state: "RUNNING",
    });
    for (const id of ["T1", "T2"]) {
      const detail = (await call("GET", `/tasks/${id}`)).body;
      expect(validateFixture(detail, props.task_detail), id).toEqual([]);
      expect(detail.task).toMatchObject({ id, status: "running", skills: ["browser-enabled"] });
    }
    const list = (await call("GET", "/tasks?limit=200&status=draft,ready,running&archived=true")).body;
    expect(validateFixture(list, props.task_list)).toEqual([]);
    expect(list.items.map((t: { id: string }) => t.id)).toEqual(["T1", "T2"]);
    const waits = (await call("GET", "/tasks/T1/browser/waits")).body;
    expect(validateFixture(waits, props.browser_wait_list)).toEqual([]);
    const pending = (await call("GET", "/browser/waits")).body;
    expect(validateFixture(pending, props.browser_pending_list)).toEqual([]);
    expect(pending.items.map((i: { wait: { wait_id: string } }) => i.wait.wait_id)).toEqual(["W1", "W2"]);
    const inbox = (await call("GET", "/inbox/items?kind=browser_wait")).body;
    expect(validateFixture(inbox, props.inbox_items)).toEqual([]);
    expect(inbox.items.map((i: { id: string }) => i.id)).toEqual(["browser_wait:W1", "browser_wait:W2"]);
    expect((await call("GET", "/inbox")).body.browser_waits).toHaveLength(2);
  });

  it("runs the control lease state machine with version, holder, expiry, disconnect and resume", async () => {
    await start();
    const route = "/tasks/T1/browser/control/R1/S1";
    const post = (command: unknown, version: number, owner = "owner-a", key: string = crypto.randomUUID()) =>
      call("POST", route, {
        assertion: assertion("T1", "R1", "S1", owner),
        command,
        expected_version: version,
        idempotency_key: key,
      });
    expect((await call("GET", route)).body).toMatchObject({ phase: "agent_running", version: 0, agent_may_act: true });
    expect((await call("GET", "/tasks/T1/browser/control/R2/S2")).status).toBe(404);
    expect((await post({ kind: "takeover", ttl_secs: 60 }, 0)).body).toEqual({ code: "invalid_transition" });
    expect((await post({ kind: "pause" }, 0)).body).toMatchObject({ phase: "paused", version: 1 });
    expect((await post({ kind: "takeover", ttl_secs: 60 }, 0)).body).toEqual({ code: "version_conflict" });
    const taken = await post({ kind: "takeover", ttl_secs: 60 }, 1, "owner-a", "same-key");
    expect(taken.body).toMatchObject({ phase: "human_control", version: 2, lease_holder: "owner-a" });
    expect(taken.body.lease_expires_at).toBe(Math.floor(clock / 1000) + 60);
    expect((await post({ kind: "takeover", ttl_secs: 60 }, 1, "owner-a", "same-key")).body.version).toBe(2);
    expect((await post({ kind: "renew", ttl_secs: 60 }, 2, "owner-b")).status).toBe(403);
    // 切断（disconnect）は人の lease を外して paused に落とす。agent は再開しない。
    const other = await call("POST", `${route}/disconnect`, { assertion: assertion("T1", "R1", "S1", "owner-b") });
    expect(other.body.phase).toBe("human_control");
    const gone = await call("POST", `${route}/disconnect`, { assertion: assertion("T1", "R1", "S1") });
    expect(gone.body).toMatchObject({ phase: "paused", version: 3, lease_holder: null, agent_may_act: false });
    expect((await post({ kind: "takeover", ttl_secs: 30 }, 3)).body.phase).toBe("human_control");
    clock += 31_000;
    expect((await call("GET", route)).body).toMatchObject({ phase: "paused", version: 5, lease_holder: null });
    expect((await post({ kind: "resume", fresh_snapshot: true, policy_origin_ok: false }, 5)).status).toBe(422);
    const resumed = await post({ kind: "resume", fresh_snapshot: true, policy_origin_ok: true }, 5);
    expect(resumed.body).toMatchObject({ phase: "agent_running", version: 6 });
    daemon?.browser?.setControl("T1", "R1", "S1", { auth_section: true });
    expect((await post({ kind: "pause" }, 6)).body).toEqual({ code: "auth_section_active" });
    const forged = await call("POST", route, {
      assertion: { ...assertion("T1", "R1", "S1"), signature: "00" },
      command: { kind: "stop" },
      expected_version: 6,
      idempotency_key: "k",
    });
    expect(forged.status).toBe(403);
    expect(daemon?.browser?.records.disconnect).toEqual([
      { key: "T1/R1/S1", holder: "owner-b" },
      { key: "T1/R1/S1", holder: "owner-a" },
    ]);
  });

  it("records wait decisions and credentials and resolves the inbox items", async () => {
    await start();
    const decision = {
      decision: "approve_once",
      expected_version: 1,
      idempotency_key: "idem",
      attestation: assertion("T1", "R1", "S1"),
    };
    expect((await call("POST", "/tasks/T1/browser/waits/W1/credential", decision)).status).toBe(409);
    const approved = await call("POST", "/tasks/T1/browser/waits/W1/decision", decision);
    expect(validateFixture(approved.body, props.browser_wait_result)).toEqual([]);
    expect(approved.body.wait).toMatchObject({ state: "approved", version: 2 });
    expect((await call("POST", "/tasks/T1/browser/waits/W1/decision", decision)).status).toBe(409);
    const credential = {
      expected_version: 1,
      username: "alice",
      password: "pw-secret",
      attestation: assertion("T2", "R2", "S2"),
    };
    expect((await call("POST", "/tasks/T2/browser/waits/W2/credential", credential)).body.wait.state).toBe(
      "registered",
    );
    expect(daemon?.browser?.records.waits.map((r) => [r.wait_id, r.kind, r.body.decision ?? r.body.username])).toEqual([
      ["W1", "credential", "approve_once"],
      ["W1", "decision", "approve_once"],
      ["W1", "decision", "approve_once"],
      ["W2", "credential", "alice"],
    ]);
    expect(daemon?.browser?.records.waits[3].body.password).toBe("pw-secret");
    expect((await call("GET", "/inbox/items?kind=browser_wait")).body.items).toEqual([]);
    expect((await call("GET", "/browser/waits")).body.items).toEqual([]);
  });

  it("issues live grants, checks them and replays scrubbed events from last_seen", async () => {
    await start({ credentialWait: false });
    expect(daemon?.browser?.waits.map((w) => w.wait_id)).toEqual(["W1"]);
    const base = "/tasks/T1/browser/live/R1/S1";
    const grant = (await call("POST", `${base}/grant`, { assertion: assertion("T1", "R1", "S1") })).body;
    expect(grant).toEqual({ grant_id: "G1", expires_at: Math.floor(clock / 1000) + 60 });
    const body = { assertion: assertion("T1", "R1", "S1"), grant_id: grant.grant_id };
    expect((await call("POST", `${base}/check`, body)).body).toEqual({ connected: true });
    expect((await call("POST", "/tasks/T2/browser/live/R2/S2/check", body)).status).toBe(403);
    const all = (await call("POST", `${base}/read?after=0`, body)).body;
    expect(all.plan).toEqual({ kind: "replay", after_seq: 0 });
    expect(all.events.map((e: { body: { kind: string } }) => e.body.kind)).toEqual([
      "status",
      "tabs",
      "url",
      "console",
    ]);
    daemon?.browser?.appendLive("T1", "R1", "S1", { kind: "status", state: "paused" });
    expect((await call("POST", `${base}/read?after=4`, body)).body.events).toEqual([
      { seq: 5, body: { kind: "status", state: "paused" } },
    ]);
    expect((await call("POST", `${base}/read`, body)).body).toEqual({
      plan: { kind: "reset", latest_seq: 5 },
      events: [],
    });
    clock += 61_000;
    expect((await call("POST", `${base}/check`, { ...body, assertion: assertion("T1", "R1", "S1") })).body).toEqual({
      code: "grant_expired",
    });
  });

  it("lists, registers, revokes, restores and deletes identities per project", async () => {
    await start();
    const list = async (project: string) =>
      (await call("GET", `/browser/identities?project_id=${project}`)).body.identities.map(
        (i: { identity_id: string; state: string; generation: number }) =>
          `${i.identity_id}:${i.state}:${i.generation}`,
      );
    expect(await list("P1")).toEqual(["ID1:active:1", "ID2:revoked:2"]);
    expect(await list("P2")).toEqual(["ID3:active:1"]);
    const input = {
      identity_id: "ID4",
      project_id: "P1",
      origin: "https://new.example.com",
      demand_confirmed_by: "owner",
      ttl_secs: 604800,
      state: { entries: [] },
    };
    expect((await call("POST", "/browser/identities", { ...input, origin: "http://x" })).status).toBe(422);
    expect((await call("POST", "/browser/identities", input)).status).toBe(201);
    expect((await call("POST", "/browser/identities", input)).status).toBe(409);
    expect((await call("POST", "/browser/identities/ID4/revoke", {})).body.identity).toMatchObject({
      state: "revoked",
      generation: 2,
    });
    const restore = { project_id: "P1", origin: "https://billing.example.com", session_id: null };
    expect((await call("POST", "/browser/identities/ID1/restore", restore)).status).toBe(204);
    expect((await call("POST", "/browser/identities/ID4/restore", restore)).status).toBe(409);
    expect((await call("DELETE", "/browser/identities/ID2")).status).toBe(200);
    expect(await list("P1")).toEqual(["ID1:active:1", "ID4:revoked:2"]);
    expect(daemon?.browser?.records.identities.filter((r) => r.method !== "GET").map((r) => r.path)).toEqual([
      "/api/v1/browser/identities",
      "/api/v1/browser/identities",
      "/api/v1/browser/identities",
      "/api/v1/browser/identities/ID4/revoke",
      "/api/v1/browser/identities/ID1/restore",
      "/api/v1/browser/identities/ID4/restore",
      "/api/v1/browser/identities/ID2",
    ]);
  });
});
