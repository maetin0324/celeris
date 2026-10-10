// 2026-10-10 本番の Live View 不達の再現（task 01M4GYJ3XGJNWZQDF35F1MDE0H、release 342ec107）。
// 本番の構成: dashboard upstream（CELERIS_WEB_LIVE_VIEW_UPSTREAM）は無く、frame 経路だけがある。task は過去に
// credential を使っていて、解決済み（registered・resumed）の credential 待ちが残っている。
// 修正前: grant は frames_available=true で link を返すのに、frame の WebSocket は guard が解決済みの待ちを
// 認証区間と取り違えて 409 auth_interval で拒否し、拒否は log にも残らなかった。grant が拒否されたときは
// /browser/runs が relay_unavailable に落ちて、本当の理由が見えなかった。
import assert from "node:assert/strict";
import { generateKeyPairSync, randomBytes } from "node:crypto";
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-browser-live-prod-"));
const socketDir = mkdtempSync(
  path.join(Buffer.byteLength(path.join(tmpdir(), "cbp-XXXXXX", "owner.sock")) <= 107 ? tmpdir() : "/tmp", "cbp-"),
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

// 本番の task の待ちの形（/api/v1/tasks/{id}/browser/waits は解決済みの待ちも返す）。
const resolvedCredentialWaits = [
  {
    task_id: "T1",
    wait_id: "W_OLD_AUTH",
    run_id: "R0",
    reason: "waiting_for_auth",
    state: "registered",
    credential: { credential_id: "C1" },
    version: 2,
    policy_hash: "sha256:abc",
  },
  {
    task_id: "T1",
    wait_id: "W_OLD_USE",
    run_id: "R0",
    reason: "waiting_for_approval",
    state: "resumed",
    credential: { credential_id: "C1" },
    operation: { action: "credential_use" },
    version: 3,
    policy_hash: "sha256:abc",
  },
];
let otherTaskOpenAuth = false;
let grantProblem = null;
const logs = [];
const daemon = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => {
    body += chunk;
  });
  req.on("end", () => {
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
                browser: { task_id: "T1", run_id: "R1", session_id: "S1", state: "RUNNING", live_view_url: null },
              },
            },
          ],
          has_more: false,
        }),
      );
    if (req.url === "/api/v1/tasks/T1/browser/waits")
      return res.end(JSON.stringify({ items: resolvedCredentialWaits }));
    if (req.url === "/api/v1/tasks/T2/browser/waits")
      return res.end(
        JSON.stringify({
          items: otherTaskOpenAuth
            ? [{ task_id: "T2", wait_id: "W2", run_id: "R9", reason: "waiting_for_auth", state: "pending" }]
            : [],
        }),
      );
    if (req.url === "/api/v1/tasks?limit=200&archived=true&order=created_desc")
      return res.end('{"items":[{"id":"T1"}],"next_cursor":null}');
    if (req.url?.startsWith("/api/v1/tasks?"))
      return res.end(
        JSON.stringify({
          items: [
            { id: "T1", status: "running" },
            { id: "T2", status: "blocked" },
          ],
          next_cursor: null,
        }),
      );
    if (req.url === "/api/v1/tasks/T1/browser/live/R1/S1/grant") {
      if (grantProblem) {
        res.statusCode = grantProblem.status;
        return res.end(JSON.stringify({ code: grantProblem.code }));
      }
      return res.end(JSON.stringify({ grant_id: "G1", expires_at: 9999999999, frames_available: true }));
    }
    if (req.url === "/api/v1/tasks/T1/browser/live/R1/S1/check") return res.end('{"connected":true}');
    if (req.url === "/api/v1/tasks/T1/browser/live/R1/S1/frames" && req.method === "POST") {
      res.statusCode = 200;
      res.setHeader("cache-control", "no-store");
      res.setHeader("content-type", "application/octet-stream");
      const image = Buffer.from("opaque-frame");
      const prefix = Buffer.alloc(4);
      prefix.writeUInt32BE(image.length);
      res.write(Buffer.concat([prefix, image]));
      // 画面が変わらない間 screencast は frame を出さない。接続の期限（試験では 200ms）より長く黙ってから次の frame。
      const later = Buffer.from("second-frame");
      const laterPrefix = Buffer.alloc(4);
      laterPrefix.writeUInt32BE(later.length);
      const timer = setTimeout(() => {
        if (!res.destroyed) res.write(Buffer.concat([laterPrefix, later]));
      }, 600);
      res.on("close", () => clearTimeout(timer));
      return;
    }
    res.statusCode = 404;
    res.end('{"code":"not_found"}');
  });
});
const servers = [daemon];
let base;
let gateway;
let cookie;
function get(route, options = {}) {
  return fetch(`${base}${route}`, {
    ...options,
    headers: { ...(cookie ? { Cookie: cookie } : {}), ...options.headers },
  });
}
before(async () => {
  daemon.listen(0, "127.0.0.1");
  await new Promise((resolve) => daemon.once("listening", resolve));
  const app = createApp({
    distDir: dir,
    passwordFile,
    daemonUrl: `http://127.0.0.1:${daemon.address().port}`,
    daemonTokenFile: tokenFile,
    liveUpstream: undefined, // 本番: dashboard upstream は無い
    attestationKeyFile: keyFile,
    ownerSocket: path.join(socketDir, "owner.sock"),
    log: (entry) => logs.push(entry),
    liveFrameConnectTimeoutMs: 200,
  });
  gateway = app.listen(0, "127.0.0.1");
  gateway.on("upgrade", app.locals.browserLiveUpgrade);
  servers.push(gateway);
  base = await new Promise((resolve) =>
    gateway.once("listening", () => resolve(`http://127.0.0.1:${gateway.address().port}`)),
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
});
after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
});

/** frame の WebSocket を開き、101 と `until` を含む frame、または拒否・切断までの応答を返す。 */
function openFrames(until = "opaque-frame") {
  return new Promise((resolve, reject) => {
    const socket = connect(gateway.address().port, "127.0.0.1");
    const chunks = [];
    const timer = setTimeout(() => reject(new Error("frame websocket timed out")), 3000);
    const done = (value) => {
      clearTimeout(timer);
      socket.destroy();
      resolve(value);
    };
    socket.on("error", reject);
    socket.on("data", (chunk) => {
      chunks.push(chunk);
      const all = Buffer.concat(chunks);
      if (all.includes(Buffer.from(until))) done(all.toString("latin1"));
    });
    socket.on("end", () => done(Buffer.concat(chunks).toString("latin1")));
    socket.on("connect", () =>
      socket.write(
        `GET /browser/live/T1/R1/frames HTTP/1.1\r\nHost: 127.0.0.1:${gateway.address().port}\r\nOrigin: ${base}\r\nCookie: ${cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ${randomBytes(16).toString("base64")}\r\n\r\n`,
      ),
    );
  });
}

test("browser_live_frame_resolved_credential_waits_do_not_block_frames", async () => {
  const item = (await (await get("/browser/runs?task_id=T1")).json()).items[0];
  assert.deepEqual(item.live, { state: "link", href: "/browser/live/T1/R1" });
  const reply = await openFrames();
  assert.match(reply, /101 Switching Protocols/);
  assert.ok(reply.includes("opaque-frame"));
  assert.ok(logs.some((e) => e.path === "/browser/live/frames" && e.status === 101));
});

test("browser_live_view_open_auth_wait_reports_auth_interval_not_relay_unavailable", async () => {
  otherTaskOpenAuth = true;
  try {
    const item = (await (await get("/browser/runs?task_id=T1")).json()).items[0];
    assert.deepEqual(item.live, { state: "disabled", reason: "auth_interval" });
    assert.equal("live_path" in item, false);
    const reply = await openFrames();
    assert.match(reply, /409 Rejected/);
    assert.match(reply, /"code":"auth_interval"/);
    assert.ok(
      logs.some((e) => e.path === "/browser/live/frames" && e.status === 409 && e.code === "auth_interval"),
      "a refused frame upgrade leaves its code in the log",
    );
  } finally {
    otherTaskOpenAuth = false;
  }
});

test("browser_live_view_grant_refusal_code_is_exposed", async () => {
  for (const [problem, reason] of [
    [{ status: 410, code: "run_ended" }, "run_ended"],
    [{ status: 403, code: "observation_stopped" }, "observation_stopped"],
    [{ status: 403, code: "live_view_disabled" }, "live_view_disabled"],
    [{ status: 403, code: "not_owner_session" }, "not_owner_session"],
    [{ status: 500, code: "internal" }, "grant_denied"],
  ]) {
    grantProblem = problem;
    const item = (await (await get("/browser/runs?task_id=T1")).json()).items[0];
    assert.deepEqual(item.live, { state: "disabled", reason }, JSON.stringify(problem));
  }
  grantProblem = null;
});

test("browser_live_frame_stream_survives_idle_longer_than_connect_timeout", async () => {
  await get("/browser/runs?task_id=T1"); // owner grant/binding
  const reply = await openFrames("second-frame");
  assert.match(reply, /101 Switching Protocols/);
  assert.ok(reply.includes("second-frame"), "the stream must not be cut while the page shows no change");
});
