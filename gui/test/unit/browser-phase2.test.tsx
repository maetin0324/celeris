import { generateKeyPairSync, type KeyObject, verify } from "node:crypto";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { renderToStaticMarkup } from "react-dom/server";
import { createRoutesStub, RouterContextProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type AuthConfig, authCheck, issueSessionCookie, readValidSession, setAuthConfigForTest } from "~/auth.server";
import { setAttestationKeyForTest } from "~/browser-attestation.server";
import {
  approveOwnerChallenge,
  browserOwnerView,
  handleOwnerControlLine,
  issueOwnerChallenge,
  ownerCsrfToken,
  ownerSessionHash,
  resetOwnerStoreForTest,
  revokeOwnerSession,
  startOwnerControlSocket,
} from "~/browser-owner.server";
import { liveViewRelayAvailable, runLiveViewRoute, setLiveViewRelayForTest } from "~/celeris/browser-live.server";
import { runCredentialAction, runDecisionAction, runOwnerChallengeAction } from "~/celeris/browser-waits.server";
import { CelerisClient } from "~/celeris/client.server";
import { loadTaskDetail } from "~/celeris/task-detail.server";
import type { BrowserRun, BrowserWait, EventsPage, TaskDetail } from "~/celeris/types";
import { BrowserWaitsPanel } from "~/components/BrowserWaitsPanel";
import { redactLiveViewUrls } from "~/lib/browser";
import { redactLiveViewStream } from "~/routes/events";
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

// ADR-0080 D5（登録依頼・承認）/ D6（Live View は本人の session だけ）。

const ORIGIN = "http://gui.test";
const DASHBOARD = "https://dashboard.internal.example/secret-dashboard";
const SECRET = "S3cret-Sentinel-Pa55";

const enabledAuth: AuthConfig = {
  enabled: true,
  nonLoopback: false,
  passwordDigest: Buffer.alloc(32),
  secret: "0123456789abcdef0123456789abcdef",
  secretFromFile: true,
};

let mock: MockCeleris;
let client: CelerisClient;
let publicKey: KeyObject;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
  resetOwnerStoreForTest();
  setAuthConfigForTest(enabledAuth);
  const pair = generateKeyPairSync("ed25519");
  publicKey = pair.publicKey;
  setAttestationKeyForTest(pair.privateKey);
});

afterEach(async () => {
  await mock.close();
  setAuthConfigForTest(undefined);
  setAttestationKeyForTest(undefined);
  resetOwnerStoreForTest();
});

async function loginCookie(): Promise<string> {
  const setCookie = await issueSessionCookie(enabledAuth, new Request(`${ORIGIN}/login`));
  return setCookie.split(";")[0] ?? "";
}

async function sessionHashOf(cookie: string): Promise<string> {
  const s = await readValidSession(enabledAuth, new Request(ORIGIN, { headers: { cookie } }));
  if (!s) throw new Error("no session");
  return ownerSessionHash(s.id);
}

/** この cookie session を本人にする（challenge → ローカル CLI の approve）。 */
async function makeOwner(cookie: string): Promise<string> {
  const res = await runOwnerChallengeAction(
    new Request(`${ORIGIN}/browser/owner-session`, { method: "POST", headers: { cookie, origin: ORIGIN } }),
  );
  const body = (await res.json()) as { challenge: string };
  expect(JSON.parse(handleOwnerControlLine(JSON.stringify({ op: "approve", challenge: body.challenge })))).toEqual({
    ok: true,
    code: "approved",
  });
  return ownerCsrfToken(enabledAuth, await sessionHashOf(cookie));
}

function wait(overrides: Partial<BrowserWait> = {}): BrowserWait {
  return {
    wait_id: "W1",
    task_id: "T1",
    run_id: "R1",
    session_id: "S1",
    reason: "waiting_for_auth",
    origin: "https://login.example.com",
    purpose: "社内 wiki の週報を読むため",
    credential_policy_id: "wiki-login",
    policy_revision: 3,
    policy_hash: "abcdef0123456789abcdef",
    deadline: new Date(Date.now() + 3_600_000).toISOString(),
    resume_key: "rk",
    version: 1,
    state: "pending",
    created_at: "2026-09-28T00:00:00Z",
    ...overrides,
  };
}

function serveWaits(items: BrowserWait[]) {
  mock.on("GET", "/api/v1/tasks/T1/browser/waits", (_req, res) => sendJson(res, 200, { items }));
}

function post(path: string, cookie: string | null, fields: Record<string, string>, origin: string | null = ORIGIN) {
  const headers: Record<string, string> = { "content-type": "application/x-www-form-urlencoded" };
  if (cookie) headers.cookie = cookie;
  if (origin) headers.origin = origin;
  return new Request(`${ORIGIN}${path}`, { method: "POST", headers, body: new URLSearchParams(fields).toString() });
}

function waitResult(w: BrowserWait, task_status: string) {
  return { wait: w, task_status, replayed: false };
}

describe("credential registration route (POST /browser/waits/:id/credential)", () => {
  it("forwards the secret once with a signed attestation and never echoes it", async () => {
    const cookie = await loginCookie();
    const csrf = await makeOwner(cookie);
    serveWaits([wait()]);
    mock.on("POST", "/api/v1/tasks/T1/browser/waits/W1/credential", (_req, res) =>
      sendJson(res, 200, waitResult(wait({ state: "registered", version: 2 }), "ready")),
    );
    const res = await runCredentialAction(
      client,
      post("/browser/waits/W1/credential", cookie, {
        csrf,
        task_id: "T1",
        expected_version: "1",
        username: "alice",
        password: SECRET,
        // フォームから origin/policy を差し替えようとしても使わない
        origin: "https://evil.example",
      }),
      "W1",
    );
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const text = await res.text();
    expect(JSON.parse(text)).toEqual({ ok: true, code: "registered", task_status: "ready" });
    expect(text).not.toContain(SECRET);
    expect(text).not.toContain("alice");

    const sent = mock.requests.find((r) => r.method === "POST" && r.url.endsWith("/credential"));
    const body = JSON.parse(sent?.body ?? "{}");
    expect(body.username).toBe("alice");
    expect(body.password).toBe(SECRET);
    expect(body.expected_version).toBe(1);
    expect(body).not.toHaveProperty("origin");
    const claims = JSON.parse(body.attestation.payload);
    expect(claims).toMatchObject({ task_id: "T1", wait_id: "W1", version: 1, decision: "register" });
    expect(claims.policy_hash).toBe("abcdef0123456789abcdef");
    expect(claims.owner_session_hash).toBe(await sessionHashOf(cookie));
    expect(claims.expires_at - Math.floor(Date.now() / 1000)).toBeLessThanOrEqual(30);
    expect(
      verify(null, Buffer.from(body.attestation.payload), publicKey, Buffer.from(body.attestation.signature, "hex")),
    ).toBe(true);
  });

  it("rejects unauthenticated, other sessions, missing Origin and bad CSRF before calling celeris", async () => {
    const owner = await loginCookie();
    const csrf = await makeOwner(owner);
    const other = await loginCookie();
    serveWaits([wait()]);
    const fields = { csrf, task_id: "T1", expected_version: "1", username: "u", password: SECRET };
    const cases: Array<[Request, number, string]> = [
      [post("/x", null, fields), 401, "unauthenticated"],
      [post("/x", other, fields), 403, "not_owner"],
      [post("/x", owner, fields, null), 403, "csrf_failed"],
      [post("/x", owner, fields, "https://evil.example"), 403, "csrf_failed"],
      [post("/x", owner, { ...fields, csrf: "0".repeat(64) }), 403, "csrf_failed"],
      [new Request(`${ORIGIN}/x`, { headers: { cookie: owner } }), 405, "method_not_allowed"],
    ];
    for (const [req, status, code] of cases) {
      const res = await runCredentialAction(client, req, "W1");
      expect(res.status).toBe(status);
      const text = await res.text();
      expect(JSON.parse(text)).toEqual({ ok: false, code });
      expect(text).not.toContain(SECRET);
    }
    expect(mock.requests.some((r) => r.method === "POST")).toBe(false);
  });

  it("maps stale version, closed waits and daemon errors to fixed codes without reflecting input", async () => {
    const cookie = await loginCookie();
    const csrf = await makeOwner(cookie);
    const fields = { csrf, task_id: "T1", expected_version: "1", username: "u", password: SECRET };
    serveWaits([wait({ version: 2 })]);
    expect((await runCredentialAction(client, post("/x", cookie, fields), "W1")).status).toBe(409);
    serveWaits([wait({ state: "expired" })]);
    expect((await runCredentialAction(client, post("/x", cookie, fields), "W1")).status).toBe(409);
    serveWaits([wait({ reason: "waiting_for_approval" })]);
    expect((await runCredentialAction(client, post("/x", cookie, fields), "W1")).status).toBe(409);
    serveWaits([wait()]);
    mock.on("POST", "/api/v1/tasks/T1/browser/waits/W1/credential", (_req, res) =>
      sendProblem(res, { status: 422, code: "credential_rejected", detail: `echo ${SECRET}` }),
    );
    const res = await runCredentialAction(client, post("/x", cookie, fields), "W1");
    expect(res.status).toBe(422);
    expect(await res.text()).not.toContain(SECRET);
  });

  it("refuses to act without the GUI attestation key", async () => {
    const cookie = await loginCookie();
    const csrf = await makeOwner(cookie);
    setAttestationKeyForTest(null);
    serveWaits([wait()]);
    const res = await runCredentialAction(
      client,
      post("/x", cookie, { csrf, task_id: "T1", expected_version: "1", username: "u", password: SECRET }),
      "W1",
    );
    expect(res.status).toBe(503);
    expect(mock.requests.some((r) => r.method === "POST")).toBe(false);
  });
});

describe("approval decision route (POST /browser/waits/:id/decision)", () => {
  const approvalWait = () =>
    wait({
      reason: "waiting_for_approval",
      credential: { credential_id: "cred-1", provider: "manual", policy_id: "wiki-login" },
      operation: { intent_id: "I1", action: "click", args_digest: "d1g3st" },
    });

  it.each([
    ["approve_once", "approved", "running"],
    ["deny", "denied", "failed"],
  ])("%s is signed for this wait/version/policy and forwarded", async (decision, code, status) => {
    const cookie = await loginCookie();
    const csrf = await makeOwner(cookie);
    serveWaits([approvalWait()]);
    mock.on("POST", "/api/v1/tasks/T1/browser/waits/W1/decision", (_req, res) =>
      sendJson(res, 200, waitResult(approvalWait(), status)),
    );
    const res = await runDecisionAction(
      client,
      post("/x", cookie, { csrf, task_id: "T1", expected_version: "1", decision }),
      "W1",
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true, code, task_status: status });
    const body = JSON.parse(mock.requests.find((r) => r.url.endsWith("/decision"))?.body ?? "{}");
    expect(body.decision).toBe(decision);
    expect(body.idempotency_key).toMatch(/^[0-9a-f]{32}$/);
    expect(JSON.parse(body.attestation.payload)).toMatchObject({ decision, version: 1, wait_id: "W1" });
  });

  it("rejects decisions from other sessions and unknown decisions", async () => {
    const owner = await loginCookie();
    const csrf = await makeOwner(owner);
    const other = await loginCookie();
    serveWaits([approvalWait()]);
    const f = { csrf, task_id: "T1", expected_version: "1", decision: "approve_once" };
    expect((await runDecisionAction(client, post("/x", other, f), "W1")).status).toBe(403);
    expect((await runDecisionAction(client, post("/x", owner, { ...f, decision: "revoke" }), "W1")).status).toBe(422);
    expect(mock.requests.some((r) => r.method === "POST")).toBe(false);
  });

  it("is refused when authentication is disabled (loopback default cannot identify the owner)", async () => {
    setAuthConfigForTest({ ...enabledAuth, enabled: false, passwordDigest: null });
    const res = await runDecisionAction(client, post("/x", null, { decision: "approve_once" }), "W1");
    expect(res.status).toBe(403);
    expect(await res.json()).toEqual({ ok: false, code: "owner_unavailable" });
  });
});

describe("owner session grant", () => {
  it("challenge is one-shot, expires, and the latest approval replaces the previous owner", async () => {
    const a = await loginCookie();
    const b = await loginCookie();
    const ha = await sessionHashOf(a);
    const hb = await sessionHashOf(b);
    const now = Date.now();
    const ca = issueOwnerChallenge(ha, now + 86_400_000, now);
    expect(approveOwnerChallenge(ca, now)).toBe("approved");
    expect(approveOwnerChallenge(ca, now)).toBe("unknown_challenge");
    const stale = issueOwnerChallenge(hb, now + 86_400_000, now);
    expect(approveOwnerChallenge(stale, now + 6 * 60_000)).toBe("expired");
    const cb = issueOwnerChallenge(hb, now + 86_400_000, now);
    expect(approveOwnerChallenge(cb, now)).toBe("approved");
    expect((await browserOwnerView(new Request(ORIGIN, { headers: { cookie: a } }))).isOwner).toBe(false);
    expect((await browserOwnerView(new Request(ORIGIN, { headers: { cookie: b } }))).isOwner).toBe(true);
    revokeOwnerSession(hb);
    expect((await browserOwnerView(new Request(ORIGIN, { headers: { cookie: b } }))).isOwner).toBe(false);
  });

  it("the local control socket is 0600 in a 0700 directory and approves a challenge", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "celeris-owner-"));
    const sock = path.join(dir, "run", "owner.sock");
    const server = startOwnerControlSocket(sock);
    try {
      await new Promise<void>((r) => (server?.listening ? r() : server?.once("listening", () => r())));
      expect(statSync(sock).mode & 0o777).toBe(0o600);
      expect(statSync(path.dirname(sock)).mode & 0o777).toBe(0o700);
      const cookie = await loginCookie();
      const hash = await sessionHashOf(cookie);
      const challenge = issueOwnerChallenge(hash, Date.now() + 3_600_000);
      const reply = await new Promise<string>((resolve, reject) => {
        const c = connect(sock);
        let out = "";
        c.setEncoding("utf8");
        c.on("data", (d: string) => {
          out += d;
        });
        c.on("end", () => resolve(out));
        c.on("error", reject);
        c.write(`${JSON.stringify({ op: "approve", challenge })}\n`);
      });
      expect(JSON.parse(reply)).toEqual({ ok: true, code: "approved" });
      expect((await browserOwnerView(new Request(ORIGIN, { headers: { cookie } }))).isOwner).toBe(true);
    } finally {
      await new Promise((r) => server?.close(r));
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("control socket lines accept only a well-formed approve", () => {
    expect(JSON.parse(handleOwnerControlLine("not json")).code).toBe("bad_request");
    expect(JSON.parse(handleOwnerControlLine('{"op":"grant","challenge":"ABCDEF012345"}')).code).toBe("bad_request");
    expect(JSON.parse(handleOwnerControlLine('{"op":"approve","challenge":"ABCDEF012345"}')).code).toBe(
      "unknown_challenge",
    );
  });
});

// ---- Live View ----

const runningTask = {
  task: { id: "T1", kind: "execute", status: "running", title: "t" },
  runs: [{ run_id: "R1" }],
} as unknown as TaskDetail;

const browserRun: BrowserRun = {
  task_id: "T1",
  run_id: "R1",
  session_id: "S1",
  state: "RUNNING",
  live_view_url: DASHBOARD,
} as BrowserRun;

const browserEvents: EventsPage = {
  has_more: false,
  items: [
    {
      id: 1,
      seq: 1,
      task_id: "T1",
      ts: "2026-09-28T00:00:00Z",
      event: { type: "browser_updated", browser: browserRun },
    },
  ],
};

function serveLiveTask() {
  mock.on("POST", "/api/v1/tasks/T1/browser/live/R1/S1/grant", (_req, res) =>
    sendJson(res, 200, { grant_id: "G1", expires_at: 4_000_000_000 }),
  );
  mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 200, { items: [{ id: "T1", status: "running" }] }));
  mock.on("GET", "/api/v1/tasks/T1", (_req, res) => sendJson(res, 200, runningTask));
  mock.on("GET", "/api/v1/tasks/T1/events", (_req, res) => sendJson(res, 200, browserEvents));
  mock.on("GET", "/api/v1/tasks/T1/artifacts", (_req, res) => sendJson(res, 200, { items: [] }));
  mock.on("GET", "/api/v1/tasks/T1/timeline", (_req, res) =>
    sendJson(res, 200, {
      task_id: "T1",
      items: [{ kind: "event", at: "x", seq: 1, event: browserEvents.items[0]?.event }],
    }),
  );
  mock.on("GET", "/api/v1/tasks/T1/comments", (_req, res) => sendJson(res, 200, { items: [] }));
  serveWaits([]);
}

describe("Live View is only reachable from the owner's authenticated session", () => {
  it("unauthenticated / other session / expired cookie get no dashboard URL from the live route", async () => {
    serveLiveTask();
    const owner = await loginCookie();
    await makeOwner(owner);
    const other = await loginCookie();
    const expired = (
      await issueSessionCookie(enabledAuth, new Request(`${ORIGIN}/login`), Date.now() - 25 * 3_600_000)
    ).split(";")[0];
    const cases: Array<[Record<string, string>, number]> = [
      [{}, 401],
      [{ cookie: other }, 403],
      [{ cookie: expired ?? "" }, 401],
      [{ cookie: other, upgrade: "websocket", connection: "Upgrade" }, 403],
    ];
    for (const [headers, status] of cases) {
      const res = await runLiveViewRoute(client, new Request(`${ORIGIN}/browser/live/T1/R1`, { headers }), "T1", "R1");
      expect(res.status).toBe(status);
      const text = await res.text();
      expect(text).not.toContain("dashboard.internal");
      expect(res.headers.get("location")).toBeNull();
    }
    // 本人は guard を通るが、relay（CELERIS_GUI_LIVE_VIEW_UPSTREAM）が未設定なら開かない（URL も Location も出さない）
    const res = await runLiveViewRoute(
      client,
      new Request(`${ORIGIN}/browser/live/T1/R1`, { headers: { cookie: owner } }),
      "T1",
      "R1",
    );
    expect(res.status).toBe(503);
    expect(await res.text()).toBe("live_view_relay_unavailable\n");
    expect(res.headers.get("location")).toBeNull();
    // 他 run への差し替えは 404
    const swapped = await runLiveViewRoute(
      client,
      new Request(`${ORIGIN}/browser/live/T1/R9`, { headers: { cookie: owner } }),
      "T1",
      "R9",
    );
    expect(swapped.status).toBe(404);
  });

  it("task loader JSON never contains the dashboard URL (auth disabled, unowned and owned sessions)", async () => {
    serveLiveTask();
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] }));
    const owner = await loginCookie();
    await makeOwner(owner);
    const other = await loginCookie();
    for (const cookie of [null, other, owner]) {
      const headers: Record<string, string> = cookie ? { cookie } : {};
      const req = new Request(`${ORIGIN}/tasks/T1`, { headers });
      const data = await loadTaskDetail(client, "T1", req, await browserOwnerView(req));
      const json = JSON.stringify(data);
      expect(json).not.toContain(DASHBOARD);
      expect(json).not.toContain("dashboard.internal");
      expect(data.liveViews.R1?.state).toBe("disabled");
    }
    const ownerDataReq = new Request(`${ORIGIN}/tasks/T1`, { headers: { cookie: owner } });
    const ownerData = await loadTaskDetail(client, "T1", ownerDataReq, await browserOwnerView(ownerDataReq));
    expect(ownerData.liveViews.R1).toEqual({ state: "disabled", reason: "relay_unavailable" });
    // relay（CELERIS_GUI_LIVE_VIEW_UPSTREAM）が設定されていれば本人には同一 origin の経路だけを出す
    setLiveViewRelayForTest({ upstream: "127.0.0.1:27849" });
    try {
      const configured = await loadTaskDetail(
        client,
        "T1",
        ownerDataReq,
        await browserOwnerView(ownerDataReq),
        liveViewRelayAvailable(),
      );
      expect(configured.liveViews.R1).toEqual({ state: "link", href: "/browser/live/T1/R1" });
      expect(JSON.stringify(configured)).not.toContain("dashboard.internal");
      expect(JSON.stringify(configured)).not.toContain("27849");
      // 他 session には relay が設定されていても出さない
      const otherReq = new Request(`${ORIGIN}/tasks/T1`, { headers: { cookie: other } });
      const otherConfigured = await loadTaskDetail(client, "T1", otherReq, await browserOwnerView(otherReq), true);
      expect(otherConfigured.liveViews.R1).toEqual({ state: "disabled", reason: "not_owner" });
    } finally {
      setLiveViewRelayForTest(undefined);
    }
    const otherDataReq = new Request(`${ORIGIN}/tasks/T1`, { headers: { cookie: other } });
    const otherData = await loadTaskDetail(client, "T1", otherDataReq, await browserOwnerView(otherDataReq));
    expect(otherData.liveViews.R1).toEqual({ state: "disabled", reason: "not_owner" });
    setAuthConfigForTest({ ...enabledAuth, enabled: false, passwordDigest: null });
    const loopbackReq = new Request(`${ORIGIN}/tasks/T1`);
    const loopback = await loadTaskDetail(client, "T1", loopbackReq, await browserOwnerView(loopbackReq));
    expect(loopback.liveViews.R1).toEqual({ state: "disabled", reason: "owner_unavailable" });
    expect(JSON.stringify(loopback)).not.toContain("dashboard.internal");
  });

  it("root auth middleware answers every /browser/* entry with 401 (no redirect, no body data)", async () => {
    for (const path of ["/browser/live/T1/R1", "/browser/live/T1/R1/assets/app.js", "/browser/waits/W1/credential"]) {
      const url = `${ORIGIN}${path}`;
      let thrown: unknown = null;
      try {
        await authCheck(
          {
            request: new Request(url),
            context: new RouterContextProvider(),
            params: {},
            url: new URL(url),
            pattern: "/",
          },
          async () => new Response("ok"),
        );
      } catch (e) {
        thrown = e;
      }
      expect(thrown).toBeInstanceOf(Response);
      expect((thrown as Response).status).toBe(401);
    }
  });

  it("the SSE relay strips live_view_url values", async () => {
    const line = `id: 1\ndata: ${JSON.stringify({ type: "browser_updated", browser: browserRun })}\n\n`;
    const half = Math.floor(line.length / 2);
    const upstream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(new TextEncoder().encode(line.slice(0, half)));
        c.enqueue(new TextEncoder().encode(line.slice(half)));
        c.close();
      },
    });
    const out = await new Response(redactLiveViewStream(upstream)).text();
    expect(out).not.toContain("dashboard.internal");
    expect(out).toContain('"live_view_url":null');
    expect(out).toContain('"session_id":"S1"');
  });

  it("redactLiveViewUrls removes nested values", () => {
    expect(JSON.stringify(redactLiveViewUrls({ a: [{ b: { live_view_url: DASHBOARD } }] }))).not.toContain(DASHBOARD);
  });
});

// ---- components ----

function renderPanel(waits: BrowserWait[], owner: Parameters<typeof BrowserWaitsPanel>[0]["owner"]): string {
  const Stub = createRoutesStub([{ path: "/", Component: () => <BrowserWaitsPanel waits={waits} owner={owner} /> }]);
  return renderToStaticMarkup(<Stub initialEntries={["/"]} />);
}

const ownerProps = { available: true, isOwner: true, challengePending: false, csrfToken: "c".repeat(64) };

describe("BrowserWaitsPanel", () => {
  it("registration form: site/purpose shown, secret fields empty and not autocompleted", () => {
    const html = renderPanel([wait()], ownerProps);
    expect(html).toContain("WAITING_FOR_AUTH");
    expect(html).toContain("https://login.example.com");
    expect(html).toContain("社内 wiki の週報を読むため");
    expect(html).toContain('action="/browser/waits/W1/credential"');
    // HTML の属性名は大文字小文字を区別しない（React 19 の static markup は `autoComplete` と書く）
    const tag = (name: string) => html.match(new RegExp(`<input[^>]*name="${name}"[^>]*>`))?.[0] ?? "";
    expect(html).toMatch(/<form[^>]*autocomplete="off"[^>]*action="\/browser\/waits\/W1\/credential"/i);
    expect(tag("password")).toMatch(/type="password"/);
    expect(tag("password")).toMatch(/autocomplete="off"/i);
    expect(tag("username")).toMatch(/autocomplete="off"/i);
    expect(tag("password")).not.toMatch(/\svalue=/);
    expect(tag("username")).not.toMatch(/\svalue=/);
    expect(html).toContain('name="csrf"');
  });

  it("approval form shows target operation and policy revision with approve/deny", () => {
    const html = renderPanel(
      [
        wait({
          reason: "waiting_for_approval",
          operation: { intent_id: "I1", action: "click", args_digest: "d1g3st" },
          credential: { credential_id: "cred-1", provider: "manual", policy_id: "p" },
        }),
      ],
      ownerProps,
    );
    expect(html).toContain("WAITING_FOR_APPROVAL");
    expect(html).toContain("click");
    expect(html).toContain("d1g3st");
    expect(html).toContain("cred-1");
    expect(html).toContain("policy revision");
    expect(html).toContain("3（abcdef012345）");
    expect(html).toContain('value="approve_once"');
    expect(html).toContain('value="deny"');
    expect(html).toContain('action="/browser/waits/W1/decision"');
  });

  it("non-owner and loopback sessions see the request but no forms", () => {
    for (const owner of [
      { available: true, isOwner: false, challengePending: false, csrfToken: null },
      { available: false, isOwner: false, challengePending: false, csrfToken: null },
    ]) {
      const html = renderPanel([wait(), wait({ wait_id: "W2", reason: "waiting_for_approval" })], owner);
      expect(html).toContain("https://login.example.com");
      expect(html).not.toContain('name="password"');
      expect(html).not.toContain('value="approve_once"');
    }
  });
});
