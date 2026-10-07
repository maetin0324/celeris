import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, randomBytes } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { parseLiveUpstream } from "./browser-live.js";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-browser-live-"));
const keyFile = path.join(dir, "key");
const passwordFile = path.join(dir, "password");
const tokenFile = path.join(dir, "token");
writeFileSync(keyFile, generateKeyPairSync("ed25519").privateKey.export({ type: "pkcs8", format: "pem" }), {
  mode: 0o600,
});
writeFileSync(passwordFile, "pw\n", { mode: 0o600 });
writeFileSync(tokenFile, "test-daemon-token", { mode: 0o600 });
chmodSync(dir, 0o700);
writeFileSync(path.join(dir, "index.html"), "<!doctype html><title>web</title>");
const requests = [];
const upgradedSockets = new Set();
let runState = "RUNNING";
let pendingWaits = false;
let leaseActive = false;
let leaseHolder = null;
let upstreamInput = null;
const daemon = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => {
    body += chunk;
  });
  req.on("end", () => {
    requests.push({ method: req.method, url: req.url, body });
    assert.equal(req.headers.authorization, "Bearer test-daemon-token");
    res.setHeader("content-type", "application/json");
    if (req.url === "/api/v1/tasks/T1")
      return res.end(
        JSON.stringify({ task: { status: "running", skills: ["browser-enabled"] }, runs: [{ run_id: "R1" }] }),
      );
    if (req.url?.startsWith("/api/v1/tasks/T1/events?"))
      return res.end(
        JSON.stringify({
          items: [
            {
              task_id: "T1",
              seq: 1,
              event: {
                type: "browser_updated",
                browser: {
                  task_id: "T1",
                  run_id: "R1",
                  session_id: "S1",
                  state: runState,
                  live_view_url: "https://secret.example/dashboard",
                },
              },
            },
          ],
          has_more: false,
        }),
      );
    if (req.url === "/api/v1/tasks/T1/browser/waits")
      return res.end(
        JSON.stringify({
          items: pendingWaits
            ? [
                {
                  task_id: "T1",
                  wait_id: "W1",
                  run_id: "R1",
                  reason: "waiting_for_approval",
                  state: "pending",
                  deadline: "2099-01-01T00:00:00Z",
                  version: 1,
                  policy_hash: "sha256:abc",
                },
                {
                  task_id: "T1",
                  wait_id: "W2",
                  run_id: "R1",
                  reason: "waiting_for_auth",
                  state: "pending",
                  deadline: "2099-01-01T00:00:00Z",
                  version: 1,
                  policy_hash: "sha256:abc",
                },
              ]
            : [],
        }),
      );
    if (req.url === "/api/v1/tasks?limit=200&archived=true&order=created_desc")
      return res.end('{"items":[{"id":"T1"}],"next_cursor":null}');
    if (req.url?.startsWith("/api/v1/tasks?")) return res.end('{"items":[],"next_cursor":null}');
    if (req.url === "/api/v1/tasks/T1/browser/live/R1/S1/grant") {
      leaseHolder = JSON.parse(JSON.parse(body).assertion.payload).owner_session_id;
      return res.end('{"grant_id":"G1","expires_at":9999999999}');
    }
    if (req.url === "/api/v1/tasks/T1/browser/live/R1/S1/check") return res.end('{"connected":true}');
    if (req.url === "/api/v1/tasks/T1/browser/control/R1/S1" && req.method === "GET")
      return res.end(
        JSON.stringify({
          phase: leaseActive ? "human_control" : "paused",
          version: 1,
          auth_section: false,
          lease_holder: leaseActive ? leaseHolder : null,
          lease_expires_at: 9999999999,
        }),
      );
    if (req.url === "/api/v1/tasks/T1/browser/control/R1/S1" && req.method === "POST")
      return res.end('{"phase":"human_control","version":2}');
    if (req.url === "/api/v1/tasks/T1/browser/control/R1/S1/disconnect") return res.end("{}");
    if (
      req.url === "/api/v1/tasks/T1/browser/waits/W1/decision" ||
      req.url === "/api/v1/tasks/T1/browser/waits/W2/credential"
    )
      return res.end('{"task_status":"running"}');
    if (req.url === "/api/v1/browser/identities?project_id=P1")
      return res.end('{"identities":[{"identity_id":"I1","project_id":"P1","origin":"https://example.com"}]}');
    if (req.url === "/api/v1/browser/identities/I1/revoke") return res.end('{"identity":{"identity_id":"I1"}}');
    res.statusCode = 404;
    res.end('{"code":"not_found"}');
  });
});
const dashboard = http.createServer((req, res) => {
  res.setHeader("set-cookie", "upstream-secret=1");
  res.setHeader("location", "https://secret.example/dashboard");
  res.setHeader("content-type", req.url === "/" ? "text/html" : "application/json");
  res.end(req.url === "/" ? "<!doctype html><title>dashboard</title>" : '{"ok":true}');
});
dashboard.on("upgrade", (req, socket) => {
  upgradedSockets.add(socket);
  socket.on("close", () => upgradedSockets.delete(socket));
  const accept = createHash("sha1")
    .update(`${req.headers["sec-websocket-key"]}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  socket.on("data", (data) => {
    upstreamInput?.(data);
  });
});
const servers = [daemon, dashboard];
let app;
let base;
let gateway;
let cookie;
let csrf;
async function listen(server) {
  server.listen(0, "127.0.0.1");
  await new Promise((resolve) => server.once("listening", resolve));
  return `http://127.0.0.1:${server.address().port}`;
}
function get(route, options = {}) {
  return fetch(`${base}${route}`, {
    ...options,
    headers: { ...(cookie ? { Cookie: cookie } : {}), ...options.headers },
  });
}
before(async () => {
  const daemonUrl = await listen(daemon);
  const liveUrl = await listen(dashboard);
  app = createApp({
    distDir: dir,
    passwordFile,
    daemonUrl,
    daemonTokenFile: tokenFile,
    liveUpstream: liveUrl.slice("http://".length),
    attestationKeyFile: keyFile,
    ownerSocket: path.join(dir, "owner.sock"),
    log: () => {},
  });
  const server = app.listen(0, "127.0.0.1");
  server.on("upgrade", app.locals.browserLiveUpgrade);
  gateway = server;
  servers.push(server);
  base = await new Promise((resolve) =>
    server.once("listening", () => resolve(`http://127.0.0.1:${server.address().port}`)),
  );
  const ownerServer = app.locals.browserLive.startSocket();
  servers.push(ownerServer);
  await new Promise((resolve) => ownerServer.once("listening", resolve));
  const login = await get("/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password: "pw" }),
  });
  cookie = login.headers.get("set-cookie").split(";")[0];
  const challenge = await (await get("/browser/owner-session", { method: "POST", headers: { Origin: base } })).json();
  const approved = await new Promise((resolve, reject) => {
    const socket = connect(path.join(dir, "owner.sock"));
    let response = "";
    socket.once("connect", () =>
      socket.write(`${JSON.stringify({ op: "approve", challenge: challenge.challenge })}\n`),
    );
    socket.on("data", (chunk) => {
      response += chunk;
    });
    socket.once("end", () => resolve(JSON.parse(response)));
    socket.once("error", reject);
  });
  assert.deepEqual(approved, { ok: true, code: "approved" });
  csrf = (await (await get("/browser/owner-session")).json()).csrfToken;
});

test("rejected upgrade RST leaves gateway serving after unauthenticated and origin mismatch requests", async () => {
  const uncaught = [];
  const onUncaught = (error) => uncaught.push(error);
  process.on("uncaughtExceptionMonitor", onUncaught);
  try {
    for (const [headers, resetOnResponse] of [
      [{ Origin: base }, false],
      [{ Origin: "http://evil.example", Cookie: cookie }, true],
    ]) {
      const socket = connect(gateway.address().port, "127.0.0.1");
      socket.on("error", () => {});
      const closed = new Promise((resolve) => socket.once("close", resolve));
      if (resetOnResponse) socket.once("data", () => socket.resetAndDestroy());
      else gateway.prependOnceListener("upgrade", () => socket.resetAndDestroy());
      await new Promise((resolve) => socket.once("connect", resolve));
      socket.write(
        `GET /api/session/1234/stream?last_seen=0 HTTP/1.1\r\nHost: 127.0.0.1:${gateway.address().port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nOrigin: ${headers.Origin}\r\n${headers.Cookie ? `Cookie: ${headers.Cookie}\r\n` : ""}\r\n` +
          "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n\r\nextra bytes",
      );
      await closed;
      assert.equal((await get("/healthz")).status, 200);
      assert.deepEqual(uncaught, []);
    }
  } finally {
    process.off("uncaughtExceptionMonitor", onUncaught);
  }
});
after(async () => {
  for (const socket of upgradedSockets) socket.destroy();
  for (const server of servers.reverse()) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

test("loopback upstream parser rejects non-loopback and URL syntax", () => {
  assert.equal(parseLiveUpstream("127.0.0.1:1234").port, 1234);
  for (const value of ["example.com:1234", "127.0.0.1:0", "http://127.0.0.1:1234", "127.0.0.1:1234/path"])
    assert.throws(() => parseLiveUpstream(value));
});
test("entry rejects missing owner, other task, nonexistent run, invalid path and raw URL", async () => {
  assert.equal((await fetch(`${base}/browser/live/T1/R1`)).status, 401);
  const other = await get("/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password: "pw" }),
  });
  assert.equal(
    (await fetch(`${base}/browser/live/T1/R1`, { headers: { Cookie: other.headers.get("set-cookie").split(";")[0] } }))
      .status,
    403,
  );
  assert.equal((await get("/browser/live/T2/R1")).status, 404);
  assert.equal((await get("/browser/live/T1/R2")).status, 404);
  assert.equal((await get("/browser/live/T1/R1/%2e%2e/api/exec")).status, 403);
  assert.equal((await get("/browser/live/T1/R1?url=https://secret.example/dashboard")).status, 400);
  assert.equal(
    (await get("/api/tasks/T1/browser/live/R1/S1/grant", { method: "POST", headers: { Origin: base } })).status,
    403,
  );
  assert.equal((await get("/api/tasks/T1/browser/control/R1/S1")).status, 403);
  assert.equal((await get("/api/browser/identities?project_id=P1")).status, 403);
});
test("entry and safe dashboard API relay with no upstream headers or raw URL", async () => {
  const entry = await get("/browser/live/T1/R1");
  assert.equal(entry.status, 200);
  assert.match(await entry.text(), /dashboard/);
  assert.equal(entry.headers.get("set-cookie"), null);
  assert.equal(entry.headers.get("location"), null);
  assert.equal(entry.headers.get("x-frame-options"), "SAMEORIGIN");
  const sub = await get("/browser/live/T1/R1/api/sessions");
  assert.equal(sub.status, 200);
  assert.equal((await sub.json()).ok, true);
  assert.equal((await get("/api/sessions")).status, 200);
  assert.equal((await get("/browser/live/T1/R1/api/exec")).status, 403);
  const listing = await (await get("/browser/runs?task_id=T1")).text();
  assert.equal(listing.includes("https://secret.example"), false);
  const all = await (await get("/browser/runs")).json();
  assert.equal(all.items[0].run_id, "R1");
  assert.equal(JSON.stringify(all).includes("https://secret.example"), false);
  const relay = await (await get("/api/tasks/T1/events?types=browser_updated")).text();
  assert.equal(relay.includes("https://secret.example"), false);
  assert.match(relay, /"live_view_url":null/);
});
test("ended run is rejected", async () => {
  runState = "COMPLETED";
  assert.equal((await get("/browser/live/T1/R1")).status, 409);
  runState = "RUNNING";
});
test("control lease acquisition and release relay with signed assertion", async () => {
  const route = "/browser/control/T1/R1/S1";
  assert.equal((await get(route)).status, 200);
  const takeover = await get(route, {
    method: "POST",
    headers: { Origin: base, "content-type": "application/json" },
    body: JSON.stringify({
      csrf,
      command: { kind: "takeover", ttl_secs: 60 },
      expected_version: 1,
      idempotency_key: "key-1",
    }),
  });
  assert.equal(takeover.status, 200);
  assert.equal((await takeover.json()).status.phase, "paused");
  const release = await get(`${route}/release`, {
    method: "POST",
    headers: { Origin: base, "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ csrf }),
  });
  assert.equal(release.status, 200);
  assert.ok(requests.some((r) => r.url?.endsWith("/disconnect") && JSON.parse(r.body).assertion.signature));
});

test("WebSocket stream denies input without a live lease", async () => {
  await get("/browser/live/T1/R1");
  const wsKey = randomBytes(16).toString("base64");
  const socket = await new Promise((resolve, reject) => {
    const request = http.request(`${base}/api/session/1234/stream?last_seen=0`, {
      headers: {
        Connection: "Upgrade",
        Upgrade: "websocket",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": wsKey,
        Origin: base,
        Cookie: cookie,
      },
    });
    request.once("upgrade", (_res, upgraded) => resolve(upgraded));
    request.once("response", (res) => reject(new Error(`upgrade rejected: ${res.statusCode}`)));
    request.once("error", reject);
    request.end();
  });
  const message = Buffer.from(JSON.stringify({ type: "input_mouse", x: 1, y: 1 }));
  const mask = randomBytes(4);
  const frame = Buffer.alloc(2 + 4 + message.length);
  frame[0] = 0x81;
  frame[1] = 0x80 | message.length;
  mask.copy(frame, 2);
  for (let i = 0; i < message.length; i++) frame[6 + i] = message[i] ^ mask[i % 4];
  const reply = new Promise((resolve, reject) => {
    socket.once("data", (data) => {
      try {
        resolve(JSON.parse(data.subarray(2).toString()));
      } catch (error) {
        reject(error);
      }
    });
    socket.once("error", reject);
  });
  socket.write(frame);
  assert.deepEqual(await reply, { type: "input_denied", code: "lease_required" });
  socket.destroy();
});

test("WebSocket stream forwards input only while this owner holds the lease", async () => {
  await get("/browser/live/T1/R1");
  leaseActive = true;
  const socket = await new Promise((resolve, reject) => {
    const request = http.request(`${base}/browser/live/T1/R1/api/session/1234/stream?last_seen=0`, {
      headers: {
        Connection: "Upgrade",
        Upgrade: "websocket",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": randomBytes(16).toString("base64"),
        Origin: base,
        Cookie: cookie,
      },
    });
    request.once("upgrade", (_res, upgraded) => resolve(upgraded));
    request.once("response", (res) => reject(new Error(`upgrade rejected: ${res.statusCode}`)));
    request.once("error", reject);
    request.end();
  });
  const message = Buffer.from('{"type":"input_keyboard","key":"A"}');
  const mask = randomBytes(4);
  const frame = Buffer.alloc(6 + message.length);
  frame[0] = 0x81;
  frame[1] = 0x80 | message.length;
  mask.copy(frame, 2);
  for (let i = 0; i < message.length; i++) frame[6 + i] = message[i] ^ mask[i % 4];
  const forwarded = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("input not forwarded")), 2000);
    upstreamInput = (data) => {
      clearTimeout(timeout);
      resolve(data);
    };
  });
  socket.write(frame);
  assert.ok((await forwarded).includes(Buffer.from("input_")) === false, "client frame is masked upstream");
  upstreamInput = null;
  leaseActive = false;
  const denied = new Promise((resolve, reject) => {
    socket.once("data", (data) => {
      try {
        resolve(JSON.parse(data.subarray(2).toString()));
      } catch (error) {
        reject(error);
      }
    });
    socket.once("error", reject);
  });
  socket.write(frame);
  assert.deepEqual(await denied, { type: "input_denied", code: "lease_required" });
  socket.destroy();
});

test("wait decision and credential require owner CSRF and omit credentials from reply", async () => {
  pendingWaits = true;
  const payload = { csrf, task_id: "T1", expected_version: 1, decision: "approve_once" };
  const headers = { Origin: base, "content-type": "application/json" };
  assert.equal(
    (
      await get("/browser/waits/W1/decision", {
        method: "POST",
        headers,
        body: JSON.stringify({ ...payload, csrf: "wrong" }),
      })
    ).status,
    403,
  );
  const decision = await get("/browser/waits/W1/decision", { method: "POST", headers, body: JSON.stringify(payload) });
  assert.equal(decision.status, 200);
  assert.equal((await decision.json()).code, "approved");
  const credential = await get("/browser/waits/W2/credential", {
    method: "POST",
    headers,
    body: JSON.stringify({ csrf, task_id: "T1", expected_version: 1, username: "person", password: "s3cr3t" }),
  });
  assert.equal(credential.status, 200);
  assert.equal((await credential.text()).includes("s3cr3t"), false);
  assert.ok(requests.some((r) => r.url?.endsWith("/W1/decision") && JSON.parse(r.body).attestation.signature));
  pendingWaits = false;
});

test("identity mutations are owner-bound and project-scoped", async () => {
  const list = await get("/browser/identities?project_id=P1");
  assert.equal(list.status, 200);
  assert.equal((await list.json()).identities[0].identity_id, "I1");
  const headers = { Origin: base, "content-type": "application/json" };
  assert.equal(
    (
      await get("/browser/identities/I1/revoke", {
        method: "POST",
        headers,
        body: JSON.stringify({ csrf, project_id: "P2" }),
      })
    ).status,
    404,
  );
  assert.equal(
    (
      await get("/browser/identities/I1/revoke", {
        method: "POST",
        headers,
        body: JSON.stringify({ csrf, project_id: "P1" }),
      })
    ).status,
    200,
  );
});

test("logout revokes browser owner and live binding", async () => {
  await get("/browser/live/T1/R1");
  const logout = await get("/logout", { method: "POST", headers: { Origin: base, accept: "application/json" } });
  assert.equal(logout.status, 200);
  assert.equal((await get("/browser/live/T1/R1")).status, 403);
});
