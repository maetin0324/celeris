import { afterEach, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import {
  chatInboxItemsFixture,
  chatSeedFixture,
  createFakeDaemon,
  defaultFixtures,
  richFixtures,
  validateFixture,
} from "./fake-daemon.mjs";

const props = schema.properties as Record<string, unknown>;

let daemon: ReturnType<typeof createFakeDaemon> | undefined;
afterEach(async () => {
  if (daemon) await daemon.close();
  daemon = undefined;
});

it("seeds chat threads, long history and every card kind with schema-valid responses", async () => {
  const seed = chatSeedFixture();
  expect(seed.map((row) => row.thread.kind)).toEqual(["human", "human", "inbox", "legacy"]);
  expect(seed[1].messages).toHaveLength(240);
  expect(seed.flatMap((row) => row.messages.flatMap((message) => message.cards.map((card) => card.kind)))).toEqual(
    expect.arrayContaining(["task", "decision", "question", "approval", "plan_gate", "notice", "operation"]),
  );
  // 受信箱 thread のカードは人待ちの決定・質問。
  const inboxCards =
    seed.find((row) => row.thread.kind === "inbox")?.messages.flatMap((message) => message.cards) ?? [];
  expect(inboxCards.map((card) => `${card.kind}:${card.id}:${card.state}`)).toEqual([
    "decision:decision:D1:pending",
    "question:question:Q1:pending",
  ]);
  const inboxItems = chatInboxItemsFixture();
  expect(
    validateFixture({ items: inboxItems, counts: { total: 6, by_kind: {} }, suppressed: {} }, props.inbox_items),
  ).toEqual([]);
  const answerable = seed
    .flatMap((row) => row.messages.flatMap((message) => message.cards))
    .filter((card) => ["question", "decision", "approval", "plan_gate"].includes(card.kind));
  expect(answerable.every((card) => inboxItems.some((item) => item.id === card.id))).toBe(true);
  daemon = createFakeDaemon({ inboxItems });
  const base = `${await daemon.start()}/api/v1/chat`;
  const get = async (path: string) => (await fetch(`${base}${path}`)).json();
  const list = await get("/threads?q=CoS");
  expect(validateFixture(list, schema.$defs.ChatThreadListResponse)).toEqual([]);
  expect(list.items.map((thread: { id: string }) => thread.id)).toContain("chat-main");
  expect((await get("/threads?q=進捗")).items.map((thread: { id: string }) => thread.id)).toContain("chat-main");
  const firstThread = await get("/threads?limit=1");
  expect(firstThread.next_cursor).toContain("|");
  expect((await get(`/threads?limit=1&before=${encodeURIComponent(firstThread.next_cursor)}`)).items[0].id).not.toBe(
    firstThread.items[0].id,
  );
  const page = await get("/threads/chat-history/messages?limit=20");
  expect(validateFixture(page, schema.$defs.ChatMessageListResponse)).toEqual([]);
  expect(page.items[0].seq).toBe(221);
  const older = await get(`/threads/chat-history/messages?before_seq=${page.items[0].seq}&limit=20`);
  expect(older.items.at(-1).seq).toBe(220);
});

it("supports thread and queue mutations with revisions and idempotency", async () => {
  daemon = createFakeDaemon();
  const base = `${await daemon.start()}/api/v1/chat`;
  const request = (path: string, method: string, body: unknown) =>
    fetch(`${base}${path}`, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const create = { client_thread_id: "client-1", title: "新しい相談", project_id: null };
  const first = await request("/threads", "POST", create);
  expect(first.status).toBe(201);
  const thread = (await first.json()).thread;
  expect((await request("/threads", "POST", create)).status).toBe(200);
  expect((await request(`/threads/${thread.id}`, "PATCH", { title: "変更後", expected_revision: 1 })).status).toBe(200);
  expect((await request(`/threads/${thread.id}`, "PATCH", { title: "古い", expected_revision: 1 })).status).toBe(409);
  const message = { client_message_id: "m1", text: "送信", attachment_ids: [], mode: "queue", resume_queue: false };
  const post = await request(`/threads/${thread.id}/messages`, "POST", message);
  expect(post.status).toBe(202);
  const queued = await post.json();
  expect(validateFixture(queued, schema.$defs.ChatPostMessageResponse)).toEqual([]);
  expect(queued.queue_position).toBe(1);
  expect((await request(`/threads/${thread.id}/messages`, "POST", message)).status).toBe(202);
  expect((await request(`/threads/${thread.id}/messages`, "POST", { ...message, text: "違う" })).status).toBe(409);
  expect((await fetch(`${base}/threads/${thread.id}/messages/${queued.message.id}`, { method: "DELETE" })).status).toBe(
    200,
  );
  expect((await request(`/threads/${thread.id}`, "PATCH", { status: "archived", expected_revision: 2 })).status).toBe(
    200,
  );
  expect(
    (await (await fetch(`${base}/threads?status=archived`)).json()).items.map((row: { id: string }) => row.id),
  ).toContain(thread.id);
});

it("replays chat SSE, emits live events, expires old cursors and handles attachments", async () => {
  daemon = createFakeDaemon();
  const root = await daemon.start();
  const base = `${root}/api/v1/chat`;
  const control = (path: string, body: unknown) =>
    fetch(`${root}/__fixture/chat${path}`, { method: "POST", body: JSON.stringify(body) });
  const hold = await (await control("/hold", { thread_id: "chat-main" })).json();
  const runResponse = await (await fetch(`${base}/threads/chat-main/runs/${hold.run_id}`)).json();
  expect(validateFixture(runResponse, schema.$defs.ChatRunResponse)).toEqual([]);
  const interrupt = await fetch(`${base}/threads/chat-main/messages`, {
    method: "POST",
    body: JSON.stringify({
      client_message_id: "interrupt-1",
      text: "急ぎです",
      attachment_ids: [],
      mode: "interrupt",
      resume_queue: false,
    }),
  });
  expect((await interrupt.json()).queue_position).toBe(1);
  expect((await (await fetch(`${base}/threads/chat-main/runs/${hold.run_id}`)).json()).run.state).toBe("stopping");
  const stream = await fetch(`${base}/threads/chat-main/stream?after=0`);
  expect(stream.status).toBe(200);
  const reader = stream.body?.getReader();
  if (!reader) throw new Error("missing stream");
  let initial = "";
  while (!initial.includes("event: run")) initial += new TextDecoder().decode((await reader.read()).value);
  expect(initial).toContain("event: run");
  const emitted = await (
    await control("/threads/chat-main/emit", {
      type: "text_delta",
      run_id: hold.run_id,
      data: { offset: 0, text: "途中" },
    })
  ).json();
  expect(validateFixture(emitted.event, schema.$defs.ChatEvent)).toEqual([]);
  expect(
    (await (await fetch(`${base}/threads/chat-main/runs/${hold.run_id}/events?after=0`)).json()).items.length,
  ).toBe(3);
  let live = "";
  while (!live.includes("途中")) live += new TextDecoder().decode((await reader.read()).value);
  expect(live).toContain("途中");
  await reader.cancel();
  const replay = await fetch(`${base}/threads/chat-main/stream`, { headers: { "Last-Event-ID": "1" } });
  const replayReader = replay.body?.getReader();
  expect(await replayReader?.read()).toBeDefined();
  await replayReader?.cancel();
  expect((await fetch(`${base}/threads/chat-main/stream?after=0`, { headers: { "Last-Event-ID": "1" } })).status).toBe(
    400,
  );
  await control("/threads/chat-main/expire", { before_id: emitted.event.id });
  expect((await fetch(`${base}/threads/chat-main/stream?after=0`)).status).toBe(410);
  const stop = await fetch(`${base}/threads/chat-main/stop`, {
    method: "POST",
    body: JSON.stringify({ run_id: hold.run_id }),
  });
  expect(stop.status).toBe(202);
  expect(validateFixture(await stop.json(), schema.$defs.ChatStopResponse)).toEqual([]);
  const detail = await (await fetch(`${base}/threads/chat-main`)).json();
  expect(detail.thread.queue_paused).toBe(true);
  expect(
    (
      await fetch(`${base}/threads/chat-main/resume-queue`, {
        method: "POST",
        body: JSON.stringify({ expected_revision: detail.thread.revision }),
      })
    ).status,
  ).toBe(200);
  const form = new FormData();
  form.set("client_upload_id", "upload-1");
  form.set("file", new Blob(["hello"], { type: "text/plain" }), "note.txt");
  const upload = await fetch(`${base}/threads/chat-main/attachments`, { method: "POST", body: form });
  expect(upload.status).toBe(201);
  const attachment = (await upload.json()).attachment;
  expect(validateFixture({ attachment }, schema.$defs.ChatAttachmentResponse)).toEqual([]);
  expect((await fetch(`${base}/threads/chat-main/attachments`, { method: "POST", body: form })).status).toBe(200);
  expect(await (await fetch(`${base}/attachments/${attachment.id}/content`)).text()).toBe("hello");
  expect((await fetch(`${base}/attachments/${attachment.id}/preview`)).status).toBe(404);
  expect(
    (
      await fetch(`${base}/attachments/${attachment.id}/references`, {
        method: "POST",
        body: JSON.stringify({ owner_kind: "task", owner_id: "T1", idempotency_key: "r1" }),
      })
    ).status,
  ).toBe(200);
  expect((await fetch(`${base}/attachments/${attachment.id}`, { method: "DELETE" })).status).toBe(409);
  const unusedForm = new FormData();
  unusedForm.set("client_upload_id", "unused");
  unusedForm.set("file", new Blob(["unused"], { type: "text/plain" }), "unused.txt");
  const unused = (
    await (await fetch(`${base}/threads/chat-main/attachments`, { method: "POST", body: unusedForm })).json()
  ).attachment;
  expect((await fetch(`${base}/attachments/${unused.id}`, { method: "DELETE" })).status).toBe(204);
  expect((await fetch(`${base}/attachments/${unused.id}/content`)).status).toBe(404);
  const imageForm = new FormData();
  imageForm.set("client_upload_id", "image");
  imageForm.set("file", new Blob(["original"], { type: "image/png" }), "image.png");
  const image = (
    await (await fetch(`${base}/threads/chat-main/attachments`, { method: "POST", body: imageForm })).json()
  ).attachment;
  const preview = await fetch(`${base}/attachments/${image.id}/preview`);
  expect(preview.status).toBe(200);
  expect(preview.headers.get("content-type")).toBe("image/png");
  expect(Array.from(new Uint8Array(await preview.arrayBuffer())).slice(0, 8)).toEqual([
    137, 80, 78, 71, 13, 10, 26, 10,
  ]);
});

it("answers CoS operation overrides deterministically (succeed and 409 conflict)", async () => {
  daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const root = await daemon.start();
  const overridePath = "/api/v1/cos/operations/OP1/override";
  // gateway の relay は Bearer を付ける。token の無い要求は 401。
  const request = (path: string, body: unknown, method: "GET" | "POST" = "POST") =>
    fetch(`${root}${path}`, {
      method,
      headers: { "content-type": "application/json" },
      body: method === "GET" ? undefined : JSON.stringify(body),
    });
  const tokenized = (path: string, body: unknown, method: "GET" | "POST" = "POST") =>
    fetch(`${root}${path}`, {
      method,
      headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
      body: method === "GET" ? undefined : JSON.stringify(body),
    });
  expect((await request(overridePath, { action: "revoke", reason: "誤り" })).status).toBe(401);
  // 理由の無い操作は 422（log には残さない）。
  expect((await tokenized(overridePath, { action: "revoke" })).status).toBe(422);
  const first = await (await tokenized(overridePath, { action: "revoke", reason: "誤り" })).json();
  expect(first).toMatchObject({ action: "revoke", state: "revoked", operation_id: "OP1", paused_task_ids: [] });
  // conflict に切ると 409（stale-revision）。
  await (await tokenized("/__fixture/chat/override-state", { state: "conflict" })).json();
  expect((await tokenized(overridePath, { action: "return", reason: "戻す" })).status).toBe(409);
  await (await tokenized("/__fixture/chat/override-state", { state: "succeed" })).json();
  await tokenized(overridePath, { action: "return", reason: "戻す" });
  const log = await (await tokenized("/__fixture/chat/override-log", {}, "GET")).json();
  expect(log.overrides).toEqual([
    { operation_id: "OP1", action: "revoke", reason: "誤り" },
    { operation_id: "OP1", action: "return", reason: "戻す" },
    { operation_id: "OP1", action: "return", reason: "戻す" },
  ]);
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

it("holds uploads until explicit success/failure, preserves retry ids and removes aborted uploads", async () => {
  daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const root = await daemon.start();
  const headers = { authorization: `Bearer ${FIXTURE_TOKEN}` };
  const control = async (path: string, body?: unknown) =>
    fetch(`${root}/__fixture/chat${path}`, {
      headers,
      method: body ? "POST" : "GET",
      body: body ? JSON.stringify(body) : undefined,
    });
  const upload = (id: string, signal?: AbortSignal) => {
    const form = new FormData();
    form.set("client_upload_id", id);
    form.set("file", new Blob(["hello"], { type: "text/plain" }), `${id}.txt`);
    return fetch(`${root}/api/v1/chat/threads/chat-main/attachments`, { headers, method: "POST", body: form, signal });
  };
  const pending = async () => (await (await control("/uploads")).json()).pending;
  const wait = (id: string) =>
    expect.poll(async () => (await pending()).some((item: { id: string }) => item.id === id)).toBe(true);
  expect((await control("/upload-state", { state: "invalid" })).status).toBe(422);
  expect((await control("/upload-state", { state: "hold" })).status).toBe(200);
  let settled = false;
  const first = upload("retry").then((response) => {
    settled = true;
    return response;
  });
  await wait("retry");
  expect(settled).toBe(false);
  expect(await pending()).toEqual([{ id: "retry", name: "retry.txt" }]);
  expect((await control("/uploads/retry/release", { state: "invalid" })).status).toBe(422);
  await control("/uploads/retry/release", { state: "fail" });
  expect((await first).status).toBe(500);
  expect(await pending()).toEqual([]);
  const retry = upload("retry");
  await wait("retry");
  await control("/uploads/retry/release", { state: "succeed" });
  const result = await retry;
  expect(result.status).toBe(201);
  const attachment = (await result.json()).attachment;
  await control("/upload-state", { state: "succeed" });
  expect((await (await upload("retry")).json()).attachment.id).toBe(attachment.id);
  await control("/upload-state", { state: "fail" });
  expect((await upload("failure")).status).toBe(500);
  await control("/upload-state", { state: "hold" });
  const abort = new AbortController();
  const cancelled = upload("cancel", abort.signal).catch((error: Error) => error.name);
  await wait("cancel");
  abort.abort();
  expect(await cancelled).toBe("AbortError");
  await expect.poll(pending).toEqual([]);
  expect((await control("/uploads/cancel/release", { state: "succeed" })).status).toBe(404);
});
