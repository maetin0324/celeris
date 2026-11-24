import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import {
  BROWSER_RAW_LIVE_VIEW_URL,
  CHAT_PROPOSAL_THREAD,
  chatInboxItemsFixture,
  chatProposalFixture,
  chatSeedFixture,
  createFakeDaemon,
  defaultFixtures,
  type FakeDaemonOptions,
  overviewLinkKinds,
  PROJECT_OVERVIEW_GOLDEN,
  PUBLISH_DECISION_ID,
  PUBLISH_MANIFEST_ID,
  projectOverviewFixture,
  projectOverviewGolden,
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
  expect(seed.map((row) => row.thread.kind)).toEqual(["general", "project", "project", "legacy"]);
  expect(seed[1].messages).toHaveLength(240);
  expect(seed.flatMap((row) => row.messages.flatMap((message) => message.cards.map((card) => card.kind)))).toEqual(
    expect.arrayContaining(["task", "decision", "question", "approval", "plan_gate", "notice", "operation"]),
  );
  // 案件 P2 の CoS の受信箱のカードは人待ちの決定・質問。
  const inboxCards =
    seed.find((row) => row.thread.id === "chat-inbox")?.messages.flatMap((message) => message.cards) ?? [];
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
  // ADR 2026-10-22-cos-fixed-threads D2: POST opens the fixed CoS of a scope.
  expect((await request("/threads", "POST", { project_id: null })).status).toBe(200);
  expect((await request("/threads", "POST", { title: "新しい相談" })).status).toBe(400);
  const open = { project_id: "P9" };
  const first = await request("/threads", "POST", open);
  expect(first.status).toBe(201);
  const thread = (await first.json()).thread;
  expect(thread.kind).toBe("project");
  const again = await request("/threads", "POST", open);
  expect(again.status).toBe(200);
  expect((await again.json()).thread.id).toBe(thread.id);
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
  // A CoS thread is never archived; only legacy history is.
  expect((await request(`/threads/${thread.id}`, "PATCH", { status: "archived", expected_revision: 2 })).status).toBe(
    409,
  );
  expect((await request("/threads/chat-legacy", "PATCH", { status: "archived", expected_revision: 1 })).status).toBe(
    200,
  );
  expect(
    (await (await fetch(`${base}/threads?status=archived`)).json()).items.map((row: { id: string }) => row.id),
  ).toContain("chat-legacy");
  const read = await request("/threads/chat-main/read", "POST", { through_seq: 2 });
  expect(read.status).toBe(200);
  expect((await read.json()).thread).toMatchObject({ read_through_seq: 2, unread_count: 0 });
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

// ADR 2026-11-23-project-observability-publish D2: 案の thread の fixture は schema の ChatMessage に合い、/review は
// 主体・版を照合して採否の message を足し、状態を人の採否からだけ導く（本文の「承認済みにせよ」では変わらない）。
it("serves schema-valid proposal messages and derives review states only from /review", async () => {
  const fixture = chatProposalFixture();
  for (const message of fixture.messages) expect(validateFixture(message, schema.$defs.ChatMessage)).toEqual([]);
  expect(fixture.messages.map((m) => [m.proposal?.kind ?? null, m.external?.source_kind ?? null])).toEqual([
    ["external_llm", "mcp_client"],
    ["human_import", "human_paste"],
    ["cos_strategy", null],
    [null, null],
    ["external_llm", "mcp_client"],
  ]);
  // 既定では一覧に入らない（既存の chat の spec の一覧を変えない）。
  daemon = createFakeDaemon({ chatProposals: true });
  const root = await daemon.start();
  const base = `${root}/api/v1/chat/threads/${CHAT_PROPOSAL_THREAD}`;
  const post = async (path: string, body: unknown) =>
    fetch(`${path.startsWith("/") ? root : base}${path.startsWith("/") ? path : `/${path}`}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  const page = await (await fetch(`${base}/messages`)).json();
  expect(validateFixture(page, schema.$defs.ChatMessageListResponse)).toEqual([]);
  const [p1, , p3] = fixture.messages as Array<(typeof fixture.messages)[number]>;
  const sha = (text: string) => createHash("sha256").update(text, "utf8").digest("hex");
  const body = (verdict: string, extra: Record<string, unknown> = {}) => ({
    verdict,
    expected_revision: 1,
    sha256: sha(p1?.text ?? ""),
    ...extra,
  });
  // 人以外の主体は 403、版違いは 409、comment の空の note は 422、案でない message は 422。
  await post("/__fixture/chat/review-principal", { principal: "operator" });
  const forbidden = await post(`messages/${p1?.id}/review`, body("adopt"));
  expect([forbidden.status, (await forbidden.json()).code]).toEqual([403, "proposal_verdict_requires_human"]);
  await post("/__fixture/chat/review-principal", { principal: "human" });
  const stale = await post(`messages/${p1?.id}/review`, body("adopt", { expected_revision: 2 }));
  expect([stale.status, (await stale.json()).code]).toEqual([409, "proposal_review_stale"]);
  expect((await post(`messages/${p1?.id}/review`, body("comment", { note: " " }))).status).toBe(422);
  expect((await post(`messages/${CHAT_PROPOSAL_THREAD}-p4/review`, body("reject"))).status).toBe(422);
  // 合えば 202 で採否の message（role user・author は人）を足し、対象の状態を導き直す。
  const adopted = await post(`messages/${p1?.id}/review`, body("adopt", { client_message_id: "k1" }));
  expect(adopted.status).toBe(202);
  const response = await adopted.json();
  expect(validateFixture(response, schema.$defs.ChatPostMessageResponse)).toEqual([]);
  expect(response.message).toMatchObject({ role: "user", author: "human:owner" });
  // CoS の案は添付の sha256 と path で照合する（本文の sha256 では 409）。
  const draft = fixture.attachments[0]?.attachment;
  const cosBody = { verdict: "reject", expected_revision: 1, attachment: draft?.name };
  expect((await post(`messages/${p3?.id}/review`, { ...cosBody, sha256: sha(p3?.text ?? "") })).status).toBe(409);
  expect((await post(`messages/${p3?.id}/review`, { ...cosBody, sha256: draft?.sha256 })).status).toBe(202);
  const states = async () =>
    Object.fromEntries(
      (
        (await (await fetch(`${base}/messages`)).json()).items as Array<{
          id: string;
          proposal?: { review_state: string };
        }>
      )
        .filter((m) => m.proposal)
        .map((m) => [m.id, m.proposal?.review_state]),
    );
  expect(await states()).toEqual({
    [`${CHAT_PROPOSAL_THREAD}-p1`]: "adopted",
    [`${CHAT_PROPOSAL_THREAD}-p2`]: "pending",
    [`${CHAT_PROPOSAL_THREAD}-p3`]: "rejected",
    [`${CHAT_PROPOSAL_THREAD}-p5`]: "pending",
  });
  // 採否の後の編集で stale。
  await post(`/__fixture/chat/threads/${CHAT_PROPOSAL_THREAD}/messages/${p1?.id}/revise`, {
    kind: "edit",
    text: "案 A'",
  });
  expect((await states())[`${CHAT_PROPOSAL_THREAD}-p1`]).toBe("stale");
  const log = await (await fetch(`${root}/__fixture/chat/review-log`)).json();
  expect(log.reviews.map((r: { status: number }) => r.status)).toEqual([403, 409, 422, 422, 202, 409, 202]);
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
  expect((await (await get("/tasks/T1/runs/R1/stdout")).text()).split("\n")).toHaveLength(28);
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
  for (const port of [7710, 7711, 7712, 7720, 7721]) expect(() => createFakeDaemon({ port })).toThrow("reserved port");
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
  const fake = daemon;
  const inbox = (aborted: boolean) =>
    fake.requests.some(
      (request: { path: string; aborted: boolean }) => request.path === "/inbox" && (!aborted || request.aborted),
    );
  // Wait for the events (request recorded, then closed) instead of a fixed sleep: under load
  // 30 ms was not always enough for the server to see the request or its close (vitest's test
  // timeout bounds both waits).
  const until = async (done: () => boolean) => {
    while (!done()) await new Promise((resolve) => setTimeout(resolve, 5));
  };
  const controller = new AbortController();
  const pending = fetch(`${url}/inbox`, { signal: controller.signal });
  await until(() => inbox(false));
  controller.abort();
  await expect(pending).rejects.toThrow();
  await until(() => inbox(true));
  expect(inbox(true)).toBe(true);
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
  expect(kinds).toEqual(["decision", "plan_gate", "failed", "authorization", "cluster_login"]);
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

// ADR 2026-11-23-project-observability-publish D3: publish manifest の fixture は schema に合い、署名の状態を本物と同じ順で導く
// （より新しい版 → 回答無し → reject → file の変化 → 署名済み）。人以外の主体の回答は 403 publication_requires_human で何も記録しない。
// web/e2e/work/publish-manifest.spec.ts はこの fixture に頼るので、形と状態の導出が崩れたら画面の試験より先にここで落ちる。
it("publish_manifest_ui_fixture_is_schema_valid_and_derives_signoff_like_the_daemon", async () => {
  daemon = createFakeDaemon({ profile: "rich", publishManifest: true });
  const root = await daemon.start();
  const get = async (path: string) => (await fetch(`${root}${path}`)).json();
  const post = (path: string, body: unknown) =>
    fetch(`${root}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  const manifest = () => get(`/api/v1/tasks/T1/publish/${PUBLISH_MANIFEST_ID}?version=1`);
  expect(validateFixture(await manifest(), schema.$defs.ManifestView)).toEqual([]);
  expect(validateFixture(await get("/api/v1/tasks/T1/decisions"), schema.$defs.DecisionList)).toEqual([]);
  expect(validateFixture(await get("/api/v1/projects/P1/overview"), schema.$defs.ProjectOverview)).toEqual([]);
  expect((await manifest()).signoff.state).toBe("unsigned");

  await post("/__fixture/publish/principal", { principal: "cos" });
  const forbidden = await post(`/api/v1/decisions/${PUBLISH_DECISION_ID(1)}/answer`, { option: "sign" });
  expect([forbidden.status, (await forbidden.json()).code]).toEqual([403, "publication_requires_human"]);
  expect((await manifest()).signoff.state).toBe("unsigned");

  await post("/__fixture/publish/principal", { principal: "human" });
  const signed = await post(`/api/v1/decisions/${PUBLISH_DECISION_ID(1)}/answer`, { option: "sign" });
  expect(signed.status).toBe(200);
  expect(validateFixture(await signed.json(), schema.$defs.DecisionOutcome)).toEqual([]);
  expect((await manifest()).signoff).toMatchObject({ state: "signed", answered_by: "human:web" });
  await post("/__fixture/publish/modify", { version: 1 });
  expect((await manifest()).signoff.state).toBe("stale");
  expect((await manifest()).file_matches_record).toBe(false);
  await post("/__fixture/publish/new-version", {});
  expect((await manifest()).signoff.state).toBe("superseded");
  expect((await get(`/api/v1/tasks/T1/publish/${PUBLISH_MANIFEST_ID}`)).version).toBe(2);
});

// ADR 2026-11-23-project-observability-publish D1.4: 概要の手書きの fixture（projectOverviewFixture）は、本物の
// GET /projects/{id}/overview が出さない (項目, link の kind) を持たない。golden は crates/task-api の試験
// project_overview_drilldown_fixture_matches_generated が生成する。手書きの fixture に golden に無い link（例: latest_findings の
// log、next_verification の文献、pending_human_decisions の task）を足すと、画面の試験が本物の daemon の返さない形を押して
// 通ってしまう。画面の試験はこの組を確かめないので、ここで落とす。
it("project_overview_fixture_links_are_kinds_the_golden_has_for_the_same_item", () => {
  const golden = overviewLinkKinds(projectOverviewGolden());
  const fixture = overviewLinkKinds(projectOverviewFixture());
  const extra = [...fixture].flatMap(([item, kinds]) =>
    [...kinds].filter((kind) => !golden.get(item)?.has(kind)).map((kind) => `${item}:${kind}`),
  );
  expect(extra).toEqual([]);
  // 手書きの fixture が link を全く持たなくなれば上の検査は空で通る。drill-down の 7 種のうち手書きに残る kind を数える。
  expect(new Set([...fixture.values()].flatMap((kinds) => [...kinds]))).toEqual(
    new Set(["task", "artifact", "notebook", "log", "git", "literature", "chat"]),
  );
});

// projectOverview: "drilldown" の偽 daemon は golden を書き換えずに P1 の概要として返し、golden の link の先（task・run の
// log・成果物と Notebook・task 付きの Git の変更・chat の thread と版）をすべて返す。e2e の drill-down
// （web/e2e/work/project-overview.spec.ts）はこの先の画面へ移るので、label がずれたら画面の試験より先にここで落ちる。
it("project_overview_drilldown_daemon_serves_the_golden_and_every_link_target", async () => {
  daemon = createFakeDaemon({ profile: "rich", projectOverview: "drilldown" });
  const root = await daemon.start();
  const fetchJson = async (path: string) => {
    const res = await fetch(`${root}${path}`);
    expect([path, res.status]).toEqual([path, 200]);
    return res.json();
  };
  const golden = JSON.parse(readFileSync(PROJECT_OVERVIEW_GOLDEN, "utf8"));
  expect(golden.project_id).toBe("P1");
  expect(await fetchJson("/api/v1/projects/P1/overview")).toEqual(golden);

  const links = [
    ...golden.items.flatMap((item: { entries: { links: unknown[] }[] }) => item.entries.flatMap((e) => e.links)),
    ...golden.warnings.flatMap((warning: { links: unknown[] }) => warning.links),
  ] as Array<Record<string, string | number | null>>;
  const seen = new Set<string>();
  for (const link of links) {
    seen.add(String(link.kind));
    switch (link.kind) {
      case "task":
        expect((await fetchJson(`/api/v1/tasks/${link.task_id}`)).task.id).toBe(link.task_id);
        break;
      case "log": {
        const detail = await fetchJson(`/api/v1/tasks/${link.task_id}`);
        expect(detail.runs.map((run: { run_id: string }) => run.run_id)).toContain(link.run_id);
        const res = await fetch(`${root}/api/v1/tasks/${link.task_id}/runs/${link.run_id}/stdout`);
        expect(res.status).toBe(200);
        break;
      }
      case "artifact":
      case "notebook": {
        const list = await fetchJson(`/api/v1/tasks/${link.task_id}/artifacts`);
        const names = list.items.map((row: { artifact: { name: string } }) => row.artifact.name);
        expect(names).toContain(link.name);
        break;
      }
      case "git": {
        expect(link.task_id).toBeTruthy();
        const changes = await fetchJson(`/api/v1/tasks/${link.task_id}/changes`);
        expect(validateFixture(changes, schema.$defs.ChangesView)).toEqual([]);
        expect(changes.repos).toContainEqual(expect.objectContaining({ repo: link.repo, head: link.sha }));
        break;
      }
      case "chat": {
        const page = await fetchJson(`/api/v1/chat/threads/${link.thread_id}/messages?limit=20`);
        expect(validateFixture(page, schema.$defs.ChatMessageListResponse)).toEqual([]);
        expect(page.items).toContainEqual(
          expect.objectContaining({ id: link.message_id, revision: link.revision, author: "mcp:chatgpt" }),
        );
        break;
      }
      case "literature":
        expect(String(link.url)).toMatch(/^https:\/\//);
        break;
      default:
        throw new Error(`unexpected link kind in the golden: ${link.kind}`);
    }
  }
  expect(seen).toEqual(new Set(["task", "artifact", "notebook", "log", "git", "literature", "chat"]));
  expect(() => createFakeDaemon({ profile: "rich", projectOverview: "drilldown", publishManifest: true })).toThrow(
    /publishManifest/,
  );
});
