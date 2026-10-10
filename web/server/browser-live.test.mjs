import assert from "node:assert/strict";
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, verify } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { parseLiveUpstream } from "./browser-live.js";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-browser-live-"));
const socketDir = mkdtempSync(
  path.join(Buffer.byteLength(path.join(tmpdir(), "cbl-XXXXXX", "owner.sock")) <= 107 ? tmpdir() : "/tmp", "cbl-"),
);
chmodSync(socketDir, 0o700);
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
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
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
    ownerSocket: path.join(socketDir, "owner.sock"),
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
  await new Promise((resolve, reject) => {
    ownerServer.once("listening", resolve);
    ownerServer.once("error", reject);
  });
  const login = await get("/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password: "pw" }),
  });
  cookie = login.headers.get("set-cookie").split(";")[0];
  const challenge = await (await get("/browser/owner-session", { method: "POST", headers: { Origin: base } })).json();
  const approved = await new Promise((resolve, reject) => {
    const socket = connect(path.join(socketDir, "owner.sock"));
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
  rmSync(socketDir, { recursive: true, force: true });
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

// --- 信頼できる端末（ADR 2026-10-07-browser-trusted-devices）---
// 偽の daemon は daemon の端点（crates/task-api/src/browser_trusted_devices.rs）の契約を真似る:
// Ed25519 assertion の検証、hash だけの保存、使うたびの回転、旧秘密の再提示で失効、90 日の sliding 期限。
const DAY_MS = 24 * 60 * 60 * 1000;
const devicePublicKey = createPublicKey(createPrivateKey(readFileSync(keyFile)));
const sessionSecretFile = path.join(dir, "session-secret");
writeFileSync(sessionSecretFile, "persisted-session-secret\n", { mode: 0o600 });
let clock = Date.UTC(2026, 9, 7);
const deviceRows = new Map();
const deviceRequests = [];
let deviceSeq = 0;
const deviceDaemon = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => {
    body += chunk;
  });
  req.on("end", () => {
    deviceRequests.push({ method: req.method, url: req.url, body, headers: req.headers });
    const nowSecs = Math.floor(clock / 1000);
    const reply = (status, value) => {
      res.statusCode = status;
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(value));
    };
    if (req.headers.authorization !== "Bearer test-daemon-token") return reply(401, { code: "unauthorized" });
    const parsed = body ? JSON.parse(body) : {};
    const signed = parsed.assertion ?? {
      payload: req.headers["x-celeris-assertion-payload"],
      signature: req.headers["x-celeris-assertion-signature"],
    };
    if (
      typeof signed.payload !== "string" ||
      !verify(null, Buffer.from(signed.payload), devicePublicKey, Buffer.from(signed.signature ?? "", "hex"))
    )
      return reply(403, { code: "not_owner_session" });
    const claims = JSON.parse(signed.payload);
    if (claims.expires_at < nowSecs || claims.expires_at > nowSecs + 30 || !claims.owner_session_id)
      return reply(403, { code: "not_owner_session" });
    const view = ({ secret_hash, prev_secret_hash, ...row }) => row;
    const active = (row) => !row.revoked_at && row.expires_at > nowSecs;
    if (req.url === "/api/v1/browser/trusted-devices" && req.method === "POST") {
      if (claims.purpose !== "device_register" || !claims.owner_session || claims.presented_hash !== parsed.secret_hash)
        return reply(403, { code: "not_owner_session" });
      if ([...deviceRows.values()].filter(active).length >= 5) return reply(409, { code: "device_limit" });
      const id = `01M4ADMXWYHPSVEJBJ5JPCR${String(++deviceSeq).padStart(3, "0")}`;
      const row = {
        id,
        name: parsed.name,
        method: "cookie",
        secret_hash: parsed.secret_hash,
        prev_secret_hash: null,
        created_at: nowSecs,
        last_used_at: null,
        expires_at: nowSecs + 90 * 86400,
        absolute_expires_at: null,
        revoked_at: null,
        revoked_reason: null,
        actor: claims.actor_id,
      };
      deviceRows.set(id, row);
      return reply(201, { device: view(row) });
    }
    if (req.url === "/api/v1/browser/trusted-devices/verify" && req.method === "POST") {
      if (
        claims.purpose !== "device_resume" ||
        claims.device_id !== parsed.device_id ||
        claims.presented_hash !== parsed.presented_hash ||
        claims.next_hash !== parsed.next_hash
      )
        return reply(403, { code: "not_owner_session" });
      const row = deviceRows.get(parsed.device_id);
      if (!row || !active(row)) return reply(403, { code: "device_rejected" });
      if (row.prev_secret_hash === parsed.presented_hash) {
        row.revoked_at = nowSecs;
        row.revoked_reason = "reuse";
        return reply(403, { code: "device_rejected" });
      }
      if (row.secret_hash !== parsed.presented_hash) return reply(403, { code: "device_rejected" });
      row.prev_secret_hash = row.secret_hash;
      row.secret_hash = parsed.next_hash;
      row.last_used_at = nowSecs;
      row.expires_at = nowSecs + 90 * 86400;
      return reply(200, { device: view(row) });
    }
    if (req.url === "/api/v1/browser/trusted-devices" && req.method === "GET") {
      if (claims.purpose !== "device_list" || !claims.owner_session) return reply(403, { code: "not_owner_session" });
      return reply(200, { devices: [...deviceRows.values()].map(view), limit: 5, now: nowSecs });
    }
    const revokeMatch = /^\/api\/v1\/browser\/trusted-devices\/([0-9A-Z]{26})$/.exec(req.url);
    if (revokeMatch && req.method === "DELETE") {
      if (claims.purpose !== "device_revoke" || !claims.owner_session || claims.device_id !== revokeMatch[1])
        return reply(403, { code: "not_owner_session" });
      const row = deviceRows.get(revokeMatch[1]);
      if (!row) return reply(404, { code: "device_not_found" });
      const revoked = !row.revoked_at;
      if (revoked) {
        row.revoked_at = nowSecs;
        row.revoked_reason = "owner";
      }
      return reply(200, { revoked, device: view(row) });
    }
    return reply(404, { code: "not_found" });
  });
});
let deviceDaemonUrl;
let socketSeq = 0;
const webLogs = [];

async function startWeb({ probe = false } = {}) {
  deviceDaemonUrl ??= await listen(deviceDaemon);
  const ownerSocket = path.join(socketDir, `device-owner-${++socketSeq}.sock`);
  const webApp = createApp({
    distDir: dir,
    passwordFile,
    secretFile: sessionSecretFile,
    daemonUrl: deviceDaemonUrl,
    daemonTokenFile: tokenFile,
    attestationKeyFile: keyFile,
    ownerSocket,
    probe,
    now: () => clock,
    log: (entry) => webLogs.push(entry),
  });
  const server = webApp.listen(0, "127.0.0.1");
  const url = await new Promise((resolve) =>
    server.once("listening", () => resolve(`http://127.0.0.1:${server.address().port}`)),
  );
  const ownerServer = webApp.locals.browserLive.startSocket();
  if (ownerServer) {
    await new Promise((resolve, reject) => {
      ownerServer.once("listening", resolve);
      ownerServer.once("error", reject);
    });
  }
  return {
    url,
    app: webApp,
    ownerSocket,
    ownerServer,
    async close() {
      server.closeAllConnections?.();
      await new Promise((resolve) => server.close(resolve));
      if (ownerServer) await new Promise((resolve) => ownerServer.close(resolve));
    },
  };
}
// 小さな cookie jar。login と端末の cookie を別々に持ち、要求ごとに組み合わせる。
function jarFrom(response, jar = {}) {
  for (const line of response.headers.getSetCookie()) {
    const [pair] = line.split(";");
    const name = pair.slice(0, pair.indexOf("="));
    const value = pair.slice(pair.indexOf("=") + 1);
    jar[name] = value;
    jar[`${name}:raw`] = line;
  }
  return jar;
}
function cookieHeader(jar) {
  return Object.entries(jar)
    .filter(([name, value]) => !name.endsWith(":raw") && value)
    .map(([name, value]) => `${name}=${value}`)
    .join("; ");
}
function call(web, route, jar, { method = "GET", body, headers = {} } = {}) {
  return fetch(`${web.url}${route}`, {
    method,
    headers: {
      ...(cookieHeader(jar) ? { Cookie: cookieHeader(jar) } : {}),
      ...(method === "GET" ? {} : { Origin: web.url }),
      ...(body ? { "content-type": "application/json" } : {}),
      ...headers,
    },
    body: body ? JSON.stringify(body) : undefined,
  });
}
async function login(web, jar = {}) {
  const response = await call(web, "/login", jar, { method: "POST", body: { password: "pw" } });
  assert.equal(response.status, 200);
  return jarFrom(response, jar);
}
async function ownerState(web, jar) {
  const response = await call(web, "/browser/owner-session", jar);
  assert.equal(response.status, 200);
  return response.json();
}
async function resume(web, jar) {
  const response = await call(web, "/browser/owner-session/resume", jar, { method: "POST" });
  jarFrom(response, jar);
  return response;
}
// host CLI 承認（owner socket の approve と同じ関数）で owner になり、この端末を登録する。
async function registerDevice(web, jar, name = "laptop") {
  const challenge = await (await call(web, "/browser/owner-session", jar, { method: "POST" })).json();
  assert.equal(web.app.locals.browserLive.approve(challenge.challenge), true);
  const { csrfToken } = await ownerState(web, jar);
  const response = await call(web, "/browser/trusted-devices", jar, {
    method: "POST",
    body: { name, csrf: csrfToken },
  });
  assert.equal(response.status, 201);
  const reply = await response.json();
  jarFrom(response, jar);
  return reply.device;
}
const DEVICE = "__celeris_web_device";

test("trusted_device: registered device resumes owner without a challenge after a web restart", async () => {
  clock = Date.UTC(2026, 9, 7);
  const first = await startWeb();
  const jar = await login(first);
  const device = await registerDevice(first, jar);
  const raw = jar[`${DEVICE}:raw`];
  assert.match(raw, /HttpOnly/i);
  assert.match(raw, /SameSite=Strict/i);
  assert.match(raw, /Path=\/browser\/owner-session/);
  assert.match(raw, new RegExp(`Max-Age=${90 * 86400}\\b`));
  assert.doesNotMatch(raw, /Secure/, "Secure only over https");
  const firstSecret = jar[DEVICE].split(".")[1];
  assert.equal(jar[DEVICE].split(".")[0], device.id);
  assert.ok(Buffer.from(firstSecret, "base64url").length >= 32);
  await first.close();

  // web の再起動（promote）: owner はメモリから消えるが、login と端末は残る。
  const second = await startWeb();
  const before = await ownerState(second, jar);
  assert.equal(before.isOwner, false);
  assert.equal(before.trustedDevice, true);
  assert.equal(before.resumable, true);
  const resumed = await resume(second, jar);
  assert.equal(resumed.status, 200);
  const body = await resumed.json();
  assert.equal(body.isOwner, true);
  assert.equal(body.deviceId, device.id);
  const after = await ownerState(second, jar);
  assert.equal(after.isOwner, true);
  assert.ok(after.csrfToken);
  // 回転: 新しい秘密の cookie が置き直される。
  const secondSecret = jar[DEVICE].split(".")[1];
  assert.notEqual(secondSecret, firstSecret);
  // owner で使える（一覧）。秘密も hash も応答に出ない。
  const list = await call(second, "/browser/trusted-devices", jar);
  assert.equal(list.status, 200);
  const listed = await list.text();
  assert.equal(JSON.parse(listed).currentDeviceId, device.id);
  assert.equal(listed.includes("secret_hash"), false);
  // 秘密の値は daemon への要求・log に出ない（daemon には hash だけ）。
  const seen = JSON.stringify(deviceRequests) + JSON.stringify(webLogs);
  for (const secret of [firstSecret, secondSecret]) assert.equal(seen.includes(secret), false);
  await second.close();
});

test("trusted_device: concurrent resumes from the same cookie share one daemon verify", async () => {
  clock = Date.UTC(2026, 9, 8);
  const first = await startWeb();
  const jar = await login(first);
  await registerDevice(first, jar, "tablet");
  await first.close();
  const second = await startWeb();
  const verifies = deviceRequests.filter((r) => r.url.endsWith("/verify")).length;
  const [a, b] = await Promise.all([
    call(second, "/browser/owner-session/resume", jar, { method: "POST" }),
    call(second, "/browser/owner-session/resume", jar, { method: "POST" }),
  ]);
  assert.deepEqual([a.status, b.status], [200, 200]);
  assert.equal(deviceRequests.filter((r) => r.url.endsWith("/verify")).length, verifies + 1);
  await second.close();
});

test("trusted_device: revoked device is rejected and its owner session is dropped at once", async () => {
  clock = Date.UTC(2026, 9, 9);
  const first = await startWeb();
  const jar = await login(first);
  const device = await registerDevice(first, jar, "phone");
  await first.close();
  const web = await startWeb();
  assert.equal((await resume(web, jar)).status, 200);
  const { csrfToken } = await ownerState(web, jar);
  const route = `/browser/trusted-devices/${device.id}`;
  assert.equal((await call(web, route, jar, { method: "DELETE", body: { csrf: "wrong" } })).status, 403);
  const kept = { ...jar };
  const revoked = await call(web, route, jar, { method: "DELETE", body: { csrf: csrfToken } });
  assert.equal(revoked.status, 200);
  assert.equal((await revoked.json()).revoked, true);
  assert.equal((await ownerState(web, jar)).isOwner, false, "owner from the revoked device is gone");
  assert.equal((await call(web, "/browser/trusted-devices", jar)).status, 403);
  // cookie を手元に残していても、失効した端末からは復帰できない。
  const again = await resume(web, kept);
  assert.equal(again.status, 403);
  assert.equal((await again.json()).code, "device_rejected");
  assert.match(kept[`${DEVICE}:raw`], /Expires=Thu, 01 Jan 1970/);
  assert.equal((await ownerState(web, kept)).isOwner, false);
  await web.close();
});

test("trusted_device: expiry slides with use and rejects after 90 idle days", async () => {
  clock = Date.UTC(2026, 9, 10);
  const first = await startWeb();
  let jar = await login(first);
  await registerDevice(first, jar, "desktop");
  await first.close();
  const web = await startWeb();
  // 89 日後に使うと延長される（login は 24h で切れるので取り直す）。
  clock += 89 * DAY_MS;
  jar = await login(web, jar);
  const used = await resume(web, jar);
  assert.equal(used.status, 200);
  assert.match(jar[`${DEVICE}:raw`], new RegExp(`Max-Age=${90 * 86400}\\b`));
  clock += 89 * DAY_MS;
  jar = await login(web, jar);
  assert.equal((await resume(web, jar)).status, 200);
  // 最後の使用から 90 日を越えると拒否。
  clock += 91 * DAY_MS;
  jar = await login(web, jar);
  const expired = await resume(web, jar);
  assert.equal(expired.status, 403);
  assert.equal((await ownerState(web, jar)).isOwner, false);
  await web.close();
});

test("trusted_device: wrong secret, reused old secret and malformed cookie are rejected", async () => {
  clock = Date.UTC(2026, 9, 11);
  const first = await startWeb();
  const jar = await login(first);
  const device = await registerDevice(first, jar, "work");
  await first.close();
  const web = await startWeb();
  // 誤った秘密。
  const wrong = { ...jar, [DEVICE]: `${device.id}.${randomBytes(32).toString("base64url")}` };
  assert.equal((await resume(web, wrong)).status, 403);
  // 形の不正な cookie は daemon に送らずに拒否して消す。
  const verifies = deviceRequests.filter((r) => r.url.endsWith("/verify")).length;
  const malformed = { ...jar, [DEVICE]: "not-a-device" };
  assert.equal((await resume(web, malformed)).status, 403);
  assert.match(malformed[`${DEVICE}:raw`], /Expires=Thu, 01 Jan 1970/);
  assert.equal(deviceRequests.filter((r) => r.url.endsWith("/verify")).length, verifies);
  // 正しい秘密で復帰 → 秘密が回転する。
  const stale = jar[DEVICE];
  assert.equal((await resume(web, jar)).status, 200);
  assert.equal((await ownerState(web, jar)).isOwner, true);
  // 別の login session が旧秘密を使い回す → 拒否、端末は失効し、その端末由来の owner も落ちる。
  const thief = await login(web, { [DEVICE]: stale });
  const reused = await resume(web, thief);
  assert.equal(reused.status, 403);
  assert.equal((await ownerState(web, jar)).isOwner, false);
  assert.equal(deviceRows.get(device.id).revoked_reason, "reuse");
  assert.equal((await resume(web, jar)).status, 403, "the rotated secret dies with the device");
  await web.close();
});

test("trusted_device: device cookie alone (no password login) is rejected", async () => {
  clock = Date.UTC(2026, 9, 12);
  const first = await startWeb();
  const jar = await login(first);
  const device = await registerDevice(first, jar, "shared");
  await first.close();
  const web = await startWeb();
  const verifies = deviceRequests.filter((r) => r.url.endsWith("/verify")).length;
  const onlyDevice = { [DEVICE]: jar[DEVICE] };
  const anonymous = await resume(web, onlyDevice);
  assert.equal(anonymous.status, 401);
  const forged = { [DEVICE]: jar[DEVICE], __celeris_web_session: "e30.forged" };
  assert.equal((await resume(web, forged)).status, 401);
  assert.equal((await call(web, "/browser/owner-session", onlyDevice)).status, 401);
  assert.equal(deviceRequests.filter((r) => r.url.endsWith("/verify")).length, verifies);
  // 端末は使われていないので、本人はそのまま復帰できる。
  assert.equal((await resume(web, jar)).status, 200);
  assert.equal(deviceRows.get(device.id).revoked_at, null);
  await web.close();
});

test("trusted_device: probe mode opens no owner socket and writes no device state", async () => {
  clock = Date.UTC(2026, 9, 13);
  const first = await startWeb();
  const jar = await login(first);
  const device = await registerDevice(first, jar, "probe-target");
  await first.close();
  const probe = await startWeb({ probe: true });
  assert.equal(probe.ownerServer, null);
  assert.equal(existsSync(probe.ownerSocket), false);
  const count = deviceRequests.length;
  const row = JSON.stringify(deviceRows.get(device.id));
  const state = await ownerState(probe, jar);
  assert.equal(state.resumable, false);
  const resumed = await resume(probe, jar);
  assert.equal(resumed.status, 503);
  assert.equal((await resumed.json()).code, "probe_mode");
  assert.equal(resumed.headers.getSetCookie().length, 0);
  assert.equal(deviceRequests.length, count, "probe sends nothing to the daemon");
  assert.equal(JSON.stringify(deviceRows.get(device.id)), row);
  assert.equal(await probe.app.locals.browserLive.approve("000000000000"), false);
  await probe.close();
  // probe の後も、本番の web は同じ cookie で復帰できる（回転されていない）。
  const web = await startWeb();
  assert.equal((await resume(web, jar)).status, 200);
  await web.close();
});

test("trusted_device: registration requires owner, CSRF and origin", async () => {
  clock = Date.UTC(2026, 9, 14);
  const web = await startWeb();
  const jar = await login(web);
  const notOwner = await call(web, "/browser/trusted-devices", jar, { method: "POST", body: { name: "x", csrf: "x" } });
  assert.equal(notOwner.status, 403);
  assert.equal((await notOwner.json()).code, "not_owner");
  const challenge = await (await call(web, "/browser/owner-session", jar, { method: "POST" })).json();
  web.app.locals.browserLive.approve(challenge.challenge);
  const { csrfToken } = await ownerState(web, jar);
  assert.equal(
    (await call(web, "/browser/trusted-devices", jar, { method: "POST", body: { name: "x", csrf: "bad" } })).status,
    403,
  );
  assert.equal(
    (
      await call(web, "/browser/trusted-devices", jar, {
        method: "POST",
        body: { name: "x", csrf: csrfToken },
        headers: { Origin: "http://evil.example" },
      })
    ).status,
    403,
  );
  assert.equal(
    (await call(web, "/browser/trusted-devices", jar, { method: "POST", body: { name: "", csrf: csrfToken } })).status,
    422,
  );
  await web.close();
});

after(async () => {
  deviceDaemon.closeAllConnections?.();
  if (deviceDaemonUrl) await new Promise((resolve) => deviceDaemon.close(resolve));
});
