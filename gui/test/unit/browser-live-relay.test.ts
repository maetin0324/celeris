import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo, Socket } from "node:net";
import type { Duplex } from "node:stream";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type AuthConfig, issueSessionCookie, readValidSession, setAuthConfigForTest } from "~/auth.server";
import {
  approveOwnerChallenge,
  issueOwnerChallenge,
  ownerSessionHash,
  resetOwnerStoreForTest,
  revokeOwnerSession,
} from "~/browser-owner.server";
import {
  clearLiveViewGuardCache,
  LIVE_VIEW_CSP,
  liveViewRelayAvailable,
  parseLiveViewUpstream,
  runLiveViewRoute,
  setLiveViewRelayForTest,
} from "~/celeris/browser-live.server";
import {
  allowedClientMessage,
  attachLiveViewUpgrade,
  openLiveViewConnections,
  resetLiveViewRelayConnectionsForTest,
  setLiveViewRevalidateMsForTest,
} from "~/celeris/browser-live-relay.server";
import { CelerisClient } from "~/celeris/client.server";
import type { BrowserRun, EventsPage, TaskDetail } from "~/celeris/types";
import {
  encodeWsFrame,
  openWsClient,
  WS_OP,
  type WsClientConnection,
  WsDecoder,
  type WsMessage,
  wsAcceptKey,
  wsCloseCode,
} from "~/celeris/ws-codec.server";
import { action as logoutAction } from "~/routes/logout";
import { app } from "../../server/app";
import { type MockCeleris, sendJson, startMockCeleris } from "../mock-celeris/server";

// ADR-0080 D6: 固定版 dashboard の同一 origin・読み取り専用 relay。偽の upstream（dashboard 相当の HTTP + WS）を
// loopback の ephemeral port に立て、server/app.ts の express app と `upgrade` の relay を通して確かめる。

const DASHBOARD = "https://dashboard.internal.example/secret-dashboard";

const enabledAuth: AuthConfig = {
  enabled: true,
  nonLoopback: false,
  passwordDigest: Buffer.alloc(32),
  secret: "0123456789abcdef0123456789abcdef",
  secretFromFile: true,
};

// ---- 偽の upstream（agent-browser dashboard 相当） ----

interface FakeUpstream {
  authority: string;
  requests: Array<{ method: string; url: string; host?: string; origin?: string }>;
  wsReceived: string[];
  wsSockets: Duplex[];
  execHits: number;
  close(): Promise<void>;
}

const UPSTREAM_HTML =
  '<!DOCTYPE html><html><head><script src="/_next/static/x.js"></script></head><body><script>self.__dash=1</script><div id="root">dashboard</div></body></html>';

async function startFakeUpstream(): Promise<FakeUpstream> {
  const state: FakeUpstream = {
    authority: "",
    requests: [],
    wsReceived: [],
    wsSockets: [],
    execHits: 0,
    close: async () => {},
  };
  const server = createServer((req, res) => {
    state.requests.push({
      method: req.method ?? "",
      url: req.url ?? "",
      host: req.headers.host,
      origin: req.headers.origin,
    });
    const sameOrigin = req.headers.origin === `http://${state.authority}`;
    if (req.url === "/api/exec" || req.url === "/api/kill" || req.url === "/api/chat") {
      state.execHits++;
      res.writeHead(200, { "Content-Type": "application/json" }).end('{"ok":true}');
      return;
    }
    if (req.url?.startsWith("/api/") && !sameOrigin) {
      res.writeHead(403).end("origin");
      return;
    }
    if (req.url === "/" || req.url?.startsWith("/?")) {
      res.writeHead(200, {
        "Content-Type": "text/html; charset=utf-8",
        "Set-Cookie": "__Host-agent-browser-dashboard-token=leak; Path=/",
        "X-Upstream-Secret": "tok-123",
        Location: "http://127.0.0.1:1/#dashboard-access-token=tok-123",
      });
      res.end(UPSTREAM_HTML);
      return;
    }
    if (req.url === "/_next/static/x.js") {
      res.writeHead(200, { "Content-Type": "application/javascript" }).end("console.log(1)");
      return;
    }
    if (req.url === "/api/sessions") {
      res.writeHead(200, { "Content-Type": "application/json" }).end('[{"session":"s","port":9222}]');
      return;
    }
    if (req.url === "/api/session/9222/tabs") {
      res.writeHead(200, { "Content-Type": "application/json" }).end("[]");
      return;
    }
    res.writeHead(404, { "Content-Type": "text/plain" }).end("nf");
  });
  server.on("upgrade", (req: IncomingMessage, socket: Duplex) => {
    state.requests.push({
      method: "UPGRADE",
      url: req.url ?? "",
      host: req.headers.host,
      origin: req.headers.origin,
    });
    const key = req.headers["sec-websocket-key"];
    if (req.url !== "/api/session/9222/stream" || typeof key !== "string") {
      socket.end("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
      return;
    }
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${wsAcceptKey(key)}\r\n\r\n`,
    );
    state.wsSockets.push(socket);
    const decoder = new WsDecoder({ expectMasked: true, maxMessageBytes: 1 << 20 });
    socket.on("error", () => {});
    socket.on("data", (chunk: Buffer) => {
      for (const m of decoder.push(chunk)) {
        if (m.opcode === WS_OP.TEXT) state.wsReceived.push(m.payload.toString("utf8"));
        if (m.opcode === WS_OP.CLOSE) socket.end();
      }
    });
    socket.write(encodeWsFrame(WS_OP.TEXT, Buffer.from('{"type":"frame","data":"AAAA"}')));
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  state.authority = `127.0.0.1:${(server.address() as AddressInfo).port}`;
  state.close = () =>
    new Promise<void>((r) => {
      for (const s of state.wsSockets) s.destroy();
      server.closeAllConnections();
      server.close(() => r());
    });
  return state;
}

// ---- 共通 ----

const runningTask = {
  task: { id: "T1", kind: "execute", status: "running", title: "t" },
  runs: [{ run_id: "R1" }],
} as unknown as TaskDetail;

function browserEvents(state: BrowserRun["state"]): EventsPage {
  const run = { task_id: "T1", run_id: "R1", session_id: "S1", state, live_view_url: DASHBOARD } as BrowserRun;
  return {
    has_more: false,
    items: [
      { id: 1, seq: 1, task_id: "T1", ts: "2026-09-28T00:00:00Z", event: { type: "browser_updated", browser: run } },
    ],
  };
}

let mock: MockCeleris;
let upstream: FakeUpstream;
let gui: Server;
let base: string;
let guiPort: number;
let runState: BrowserRun["state"];

beforeEach(async () => {
  runState = "RUNNING";
  mock = await startMockCeleris();
  mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 200, { items: [{ id: "T1", status: "running" }] }));
  mock.on("GET", "/api/v1/tasks/T1", (_req, res) => sendJson(res, 200, runningTask));
  mock.on("GET", "/api/v1/tasks/T1/events", (_req, res) => sendJson(res, 200, browserEvents(runState)));
  mock.on("GET", "/api/v1/tasks/T1/browser/waits", (_req, res) => sendJson(res, 200, { items: [] }));
  upstream = await startFakeUpstream();
  resetOwnerStoreForTest();
  setAuthConfigForTest(enabledAuth);
  setLiveViewRelayForTest({ upstream: upstream.authority, client: new CelerisClient({ baseUrl: mock.baseUrl }) });
  gui = createServer(app);
  attachLiveViewUpgrade(gui);
  await new Promise<void>((r) => gui.listen(0, "127.0.0.1", r));
  guiPort = (gui.address() as AddressInfo).port;
  base = `http://127.0.0.1:${guiPort}`;
});

afterEach(async () => {
  vi.restoreAllMocks();
  resetLiveViewRelayConnectionsForTest();
  setLiveViewRevalidateMsForTest(undefined);
  setLiveViewRelayForTest(undefined);
  setAuthConfigForTest(undefined);
  resetOwnerStoreForTest();
  gui.closeAllConnections();
  await new Promise<void>((r) => gui.close(() => r()));
  await upstream.close();
  await mock.close();
});

async function loginCookie(): Promise<string> {
  const setCookie = await issueSessionCookie(enabledAuth, new Request("http://gui.test/login"));
  return setCookie.split(";")[0] ?? "";
}

async function hashOf(cookie: string): Promise<string> {
  const s = await readValidSession(enabledAuth, new Request("http://gui.test/", { headers: { cookie } }));
  if (!s) throw new Error("no session");
  return ownerSessionHash(s.id);
}

async function makeOwner(cookie: string): Promise<string> {
  const hash = await hashOf(cookie);
  expect(approveOwnerChallenge(issueOwnerChallenge(hash, Date.now() + 3_600_000))).toBe("approved");
  return hash;
}

function get(path: string, cookie: string | null, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  if (cookie) headers.set("cookie", cookie);
  return fetch(`${base}${path}`, { ...init, headers, redirect: "manual" });
}

function openWs(path: string, cookie: string | null, origin: string | null = base): Promise<WsClientConnection> {
  const headers: Record<string, string> = {};
  if (cookie) headers.Cookie = cookie;
  if (origin) headers.Origin = origin;
  return openWsClient({ host: "127.0.0.1", port: guiPort, path, hostHeader: `127.0.0.1:${guiPort}`, headers });
}

/** client 側で受けた message を集める。 */
function collect(conn: WsClientConnection): { messages: WsMessage[]; ended: Promise<void> } {
  const decoder = new WsDecoder({ expectMasked: false, maxMessageBytes: 1 << 20 });
  const messages: WsMessage[] = [];
  conn.socket.on("error", () => {});
  const ended = new Promise<void>((resolve) => conn.socket.once("close", () => resolve()));
  const feed = (chunk: Buffer) => messages.push(...decoder.push(chunk));
  if (conn.leftover.length > 0) feed(conn.leftover);
  conn.socket.on("data", feed);
  return { messages, ended };
}

async function until(cond: () => boolean, ms = 3_000): Promise<void> {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > ms) throw new Error("timeout");
    await new Promise((r) => setTimeout(r, 10));
  }
}

function sendText(socket: Socket, text: string): void {
  socket.write(encodeWsFrame(WS_OP.TEXT, Buffer.from(text, "utf8"), true));
}

// ---- テスト ----

describe("Live View relay configuration", () => {
  it("accepts only loopback host:port", () => {
    expect(parseLiveViewUpstream("127.0.0.1:27849")?.authority).toBe("127.0.0.1:27849");
    expect(parseLiveViewUpstream("[::1]:27849")?.host).toBe("::1");
    expect(parseLiveViewUpstream("localhost:1")?.authority).toBe("localhost:1");
    for (const bad of [
      "10.0.0.1:80",
      "example.com:80",
      "127.0.0.1",
      "127.0.0.1:0",
      "127.0.0.1:70000",
      "http://127.0.0.1:1",
    ]) {
      expect(parseLiveViewUpstream(bad)).toBeNull();
    }
  });

  it("unset upstream keeps 503 live_view_relay_unavailable and hides the link", async () => {
    setLiveViewRelayForTest({ upstream: null });
    expect(liveViewRelayAvailable()).toBe(false);
    const owner = await loginCookie();
    await makeOwner(owner);
    const res = await runLiveViewRoute(
      new CelerisClient({ baseUrl: mock.baseUrl }),
      new Request("http://gui.test/browser/live/T1/R1", { headers: { cookie: owner } }),
      "T1",
      "R1",
    );
    expect(res.status).toBe(503);
    expect(await res.text()).toBe("live_view_relay_unavailable\n");
    await expect(openWs("/api/session/9222/stream", owner)).rejects.toThrow("ws_http_503");
  });

  it("only ack / config (stream pacing) messages are forwardable", () => {
    expect(allowedClientMessage('{"type":"ack","seq":3}')).toBe('{"type":"ack","seq":3}');
    expect(allowedClientMessage('{"type":"config","maxFps":10,"pacing":"ack"}')).toBe(
      '{"type":"config","maxFps":10,"pacing":"ack"}',
    );
    for (const bad of [
      '{"type":"input_mouse","eventType":"mousePressed","x":1,"y":1}',
      '{"type":"input_keyboard","key":"a"}',
      '{"type":"input_touch"}',
      '{"type":"config","maxFps":10,"url":"x"}',
      '{"type":"config","maxFps":1000}',
      '{"type":"ack","seq":-1}',
      '{"type":"screencast_start"}',
      "not json",
      "[]",
    ]) {
      expect(allowedClientMessage(bad)).toBeNull();
    }
  });
});

describe("Live View relay (owner)", () => {
  it("serves the dashboard HTML with relay-owned headers and never leaks upstream headers", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    const res = await get("/browser/live/T1/R1?port=9222&next=http://evil", owner);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-security-policy")).toBe(LIVE_VIEW_CSP);
    expect(res.headers.get("x-frame-options")).toBe("DENY");
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(res.headers.get("referrer-policy")).toBe("no-referrer");
    expect(res.headers.get("x-content-type-options")).toBe("nosniff");
    expect(res.headers.get("set-cookie")).toBeNull();
    expect(res.headers.get("location")).toBeNull();
    expect(res.headers.get("x-upstream-secret")).toBeNull();
    const body = await res.text();
    expect(body).toBe(UPSTREAM_HTML);
    expect(body).not.toContain(DASHBOARD);
    const seen = upstream.requests.find((r) => r.url.startsWith("/?") || r.url === "/");
    expect(seen).toMatchObject({
      url: "/?port=9222",
      host: upstream.authority,
      origin: `http://${upstream.authority}`,
    });
  });

  it("relays assets and read-only API only after the owner opened the HTML entry", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    expect((await get("/_next/static/x.js", owner)).status).toBe(404);
    expect((await get("/browser/live/T1/R1", owner)).status).toBe(200);
    const asset = await get("/_next/static/x.js", owner);
    expect(asset.status).toBe(200);
    expect(await asset.text()).toBe("console.log(1)");
    expect(asset.headers.get("content-type")).toBe("application/javascript");
    const sessions = await get("/api/sessions", owner);
    expect(sessions.status).toBe(200);
    expect(await sessions.json()).toEqual([{ session: "s", port: 9222 }]);
    expect((await get("/api/session/9222/tabs", owner)).status).toBe(200);
    // prefix 付きでも同じ規則（prefix の run は束縛と一致すること）
    expect((await get("/browser/live/T1/R1/_next/static/x.js", owner)).status).toBe(200);
    expect((await get("/browser/live/T1/R2/_next/static/x.js", owner)).status).toBe(404);
    expect((await get("/browser/live/T1/R1/api/exec", owner, { method: "POST", body: "{}" })).status).toBe(403);
  });

  it("denies every action route and non-GET method with 403 live_view_action_denied", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const cases: Array<[string, string]> = [
      ["POST", "/api/exec"],
      ["GET", "/api/exec"],
      ["POST", "/api/kill"],
      ["POST", "/api/sessions"],
      ["POST", "/api/chat"],
      ["GET", "/api/models"],
      ["GET", "/api/session/9222/stream"],
      ["GET", "/api/unknown"],
      ["PUT", "/_next/static/x.js"],
      ["GET", "/_next/data/x.json"],
      ["GET", "/_next/static/../../api/exec"],
    ];
    for (const [method, path] of cases) {
      const res = await get(path, owner, {
        method,
        headers: { origin: base, "content-type": "application/json" },
        body: method === "GET" ? undefined : '{"args":["eval","1"]}',
      });
      // `..` は URL の正規化で /api/exec になる（どちらでも拒否）
      expect([res.status, path]).toEqual([403, path]);
      expect(await res.text()).toBe("live_view_action_denied\n");
    }
    expect(upstream.execHits).toBe(0);
    expect(upstream.requests.filter((r) => r.url.includes("exec") || r.url.includes("models"))).toEqual([]);
  });

  it("relays the stream: upstream frames reach the client, only ack/config reach upstream", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.some((m) => m.opcode === WS_OP.TEXT));
    expect(client.messages[0]?.payload.toString()).toBe('{"type":"frame","data":"AAAA"}');
    sendText(conn.socket, '{"type":"input_mouse","eventType":"mousePressed","x":1,"y":1,"button":"left"}');
    sendText(conn.socket, '{"type":"input_keyboard","eventType":"keyDown","key":"a"}');
    sendText(conn.socket, '{"type":"ack","seq":1}');
    sendText(conn.socket, '{"type":"config","maxFps":5}');
    await until(() => upstream.wsReceived.length >= 2);
    await new Promise((r) => setTimeout(r, 50));
    expect(upstream.wsReceived).toEqual(['{"type":"ack","seq":1}', '{"type":"config","maxFps":5}']);
    const ws = upstream.requests.find((r) => r.method === "UPGRADE");
    expect(ws).toMatchObject({
      url: "/api/session/9222/stream",
      host: upstream.authority,
      origin: `http://${upstream.authority}`,
    });
    // ping は relay が答える
    conn.socket.write(encodeWsFrame(WS_OP.PING, Buffer.from("p"), true));
    await until(() => client.messages.some((m) => m.opcode === WS_OP.PONG));
    expect(openLiveViewConnections()).toBe(1);
    conn.socket.destroy();
  });

  it("prefixed stream path works and a cross-origin page cannot open the stream", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/browser/live/T1/R1/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    conn.socket.destroy();
    await expect(openWs("/api/session/9222/stream", owner, "http://evil.example")).rejects.toThrow("ws_http_403");
    await expect(openWs("/api/session/9222/stream", owner, null)).rejects.toThrow("ws_http_403");
    await expect(openWs("/browser/live/T1/R2/api/session/9222/stream", owner)).rejects.toThrow("ws_http_404");
  });

  it("logout closes the existing stream and drops the binding", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    await expect(
      logoutAction({
        request: new Request("http://gui.test/logout", { method: "POST", headers: { cookie: owner } }),
      } as unknown as Parameters<typeof logoutAction>[0]),
    ).rejects.toBeInstanceOf(Response);
    await client.ended;
    const close = client.messages.find((m) => m.opcode === WS_OP.CLOSE);
    expect(close && wsCloseCode(close.payload)).toBe(1001);
    expect(openLiveViewConnections()).toBe(0);
    // grant が無くなった cookie は 403、束縛も消えている
    expect((await get("/_next/static/x.js", owner)).status).toBe(403);
    await makeOwner(owner);
    expect((await get("/_next/static/x.js", owner)).status).toBe(404);
  });

  it("revokeOwnerSession (grant expiry path) closes streams too", async () => {
    const owner = await loginCookie();
    const hash = await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    revokeOwnerSession(hash);
    await client.ended;
    expect(client.messages.some((m) => m.opcode === WS_OP.CLOSE)).toBe(true);
  });

  it("an expired owner grant closes a stream even while the cookie is still valid", async () => {
    setLiveViewRevalidateMsForTest(50);
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    vi.spyOn(Date, "now").mockReturnValue(Date.now() + 3_600_001);
    await client.ended;
    expect(openLiveViewConnections()).toBe(0);
    expect((await get("/api/sessions", owner)).status).toBe(403);
  });

  it("closes the stream when the run leaves RUNNING (periodic re-validation)", async () => {
    setLiveViewRevalidateMsForTest(50);
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    runState = "FAILED";
    clearLiveViewGuardCache();
    await client.ended;
    const close = client.messages.find((m) => m.opcode === WS_OP.CLOSE);
    expect(close && wsCloseCode(close.payload)).toBe(1008);
    // 以後は asset も拒否（409 not_running）
    expect((await get("/_next/static/x.js", owner)).status).toBe(409);
  });

  it("another task's auth interval closes the entire dashboard, including an existing stream", async () => {
    // Disable the short polling path: an incoming frame must trigger its own authorization check.
    setLiveViewRevalidateMsForTest(60_000);
    const owner = await loginCookie();
    await makeOwner(owner);
    expect((await get("/browser/live/T1/R1", owner)).status).toBe(200);
    const conn = await openWs("/api/session/9222/stream", owner);
    const client = collect(conn);
    await until(() => client.messages.length > 0);
    mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 200, { items: [{ id: "T2", status: "running" }] }));
    mock.on("GET", "/api/v1/tasks/T2/browser/waits", (_req, res) =>
      sendJson(res, 200, { items: [{ run_id: "R2", reason: "waiting_for_auth", state: "registered" }] }),
    );
    upstream.wsSockets[0]?.write(encodeWsFrame(WS_OP.TEXT, Buffer.from("private auth frame")));
    for (const path of ["/browser/live/T1/R1", "/_next/static/x.js", "/api/sessions"]) {
      expect((await get(path, owner)).status).toBe(409);
    }
    await expect(openWs("/api/session/9222/stream", owner)).rejects.toThrow("ws_http_409");
    await client.ended;
    expect(client.messages.some((m) => m.opcode === WS_OP.CLOSE)).toBe(true);
    expect(client.messages.some((m) => m.payload.includes("private auth frame"))).toBe(false);
  });

  it("fails closed if namespace safety cannot be established", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 500, {}));
    expect((await get("/browser/live/T1/R1", owner)).status).toBe(503);
    mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 200, { items: [], next_cursor: "stuck" }));
    expect((await get("/browser/live/T1/R1", owner)).status).toBe(503);
    mock.on("GET", "/api/v1/tasks", (_req, res) => sendJson(res, 200, { items: [{ id: "T2", status: "running" }] }));
    mock.on("GET", "/api/v1/tasks/T2/browser/waits", (_req, res) => sendJson(res, 404, {}));
    expect((await get("/browser/live/T1/R1", owner)).status).toBe(503);
  });
});

describe("Live View relay (negative)", () => {
  it("unauthenticated gets 401 for HTML, asset, API and WS", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    for (const path of ["/browser/live/T1/R1", "/_next/static/x.js", "/api/sessions", "/api/exec"]) {
      const res = await get(path, null);
      expect([res.status, path]).toEqual([401, path]);
      expect(res.headers.get("location")).toBeNull();
    }
    await expect(openWs("/api/session/9222/stream", null)).rejects.toThrow("ws_http_401");
    const expired = (
      await issueSessionCookie(enabledAuth, new Request("http://gui.test/login"), Date.now() - 25 * 3_600_000)
    ).split(";")[0];
    expect((await get("/_next/static/x.js", expired ?? "")).status).toBe(401);
  });

  it("another logged-in session gets 403 for HTML, asset, API and WS", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    await get("/browser/live/T1/R1", owner);
    const other = await loginCookie();
    for (const path of ["/browser/live/T1/R1", "/_next/static/x.js", "/api/sessions", "/api/session/9222/tabs"]) {
      const res = await get(path, other);
      expect([res.status, path]).toEqual([403, path]);
      expect(await res.text()).toBe("not_owner\n");
    }
    await expect(openWs("/api/session/9222/stream", other)).rejects.toThrow("ws_http_403");
    expect(upstream.requests.filter((r) => r.url !== "/")).toEqual([]);
  });

  it("another run gets 404 and a stale WS path without binding gets 404", async () => {
    const owner = await loginCookie();
    await makeOwner(owner);
    expect((await get("/browser/live/T1/R9", owner)).status).toBe(404);
    expect((await get("/browser/live/T9/R1", owner)).status).toBe(404);
    await expect(openWs("/api/session/9222/stream", owner)).rejects.toThrow("ws_http_404");
    expect(upstream.requests).toEqual([]);
  });

  it("auth disabled (loopback default) refuses everything", async () => {
    setAuthConfigForTest({ ...enabledAuth, enabled: false });
    expect((await get("/browser/live/T1/R1", null)).status).toBe(403);
    expect((await get("/_next/static/x.js", null)).status).toBe(403);
    await expect(openWs("/api/session/9222/stream", null)).rejects.toThrow("ws_http_403");
  });
});
