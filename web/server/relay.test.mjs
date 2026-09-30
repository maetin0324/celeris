import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { parseUpstream, upstreamPath } from "./relay.js";

// 偽 daemon と gateway は loopback の空き port。token は fixture で、どの出力にも出てはいけない。
const TOKEN = "fixture-daemon-token-3f9c2a71";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-relay-"));
const tokenFile = path.join(dir, "token");
const passwordFile = path.join(dir, "password");
writeFileSync(tokenFile, `${TOKEN}\n`);
writeFileSync(passwordFile, "pw\n");
writeFileSync(path.join(dir, "index.html"), "<!doctype html><title>celeris</title>");

const seen = [];
let aborted = 0;
const daemon = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (chunk) => chunks.push(chunk));
  req.on("end", () => {
    seen.push({ method: req.method, url: req.url, headers: req.headers, body: Buffer.concat(chunks).toString() });
    if (req.headers.authorization !== `Bearer ${TOKEN}`) {
      res.writeHead(401, { "content-type": "application/json" });
      return res.end('{"error":"unauthorized"}');
    }
    if (req.url === "/api/v1/slow") {
      res.on("close", () => {
        if (!res.writableFinished) aborted += 1;
      });
      return;
    }
    if (req.url === "/api/v1/leak") {
      res.writeHead(500, { "content-type": "application/json", "x-secret": TOKEN });
      return res.end(JSON.stringify({ error: `bad token ${TOKEN}` }));
    }
    if (req.url === "/api/v1/missing") {
      res.writeHead(404, { "content-type": "application/json" });
      return res.end('{"error":"not found"}');
    }
    res.writeHead(200, { "content-type": "application/json", etag: '"v1"', "set-cookie": "daemon=1" });
    res.end(JSON.stringify({ ok: true, url: req.url }));
  });
});

const logs = [];
const servers = [];
let upstream;

async function listen(server) {
  server.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  servers.push(server);
  return `http://127.0.0.1:${server.address().port}`;
}

// fetch は `%2e%2e` を URL の正規化で消すので、path をそのまま送る。
function raw(base, route) {
  return new Promise((resolve, reject) => {
    const { hostname, port } = new URL(base);
    http
      .get({ hostname, port, path: route }, (res) => {
        let body = "";
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body }));
      })
      .on("error", reject);
  });
}

async function gateway(options = {}) {
  return listen(
    createApp({
      distDir: dir,
      daemonUrl: upstream,
      daemonTokenFile: tokenFile,
      log: (entry) => logs.push(JSON.stringify(entry)),
      ...options,
    }).listen(0, "127.0.0.1"),
  );
}

before(async () => {
  upstream = await listen(daemon);
});
after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
  assert.ok(!logs.join("\n").includes(TOKEN), "token leaked into request log");
});

test("relay /api/* to /api/v1/* with the daemon token only", async () => {
  const base = await gateway();
  const response = await fetch(`${base}/api/tasks/abc?limit=5`, {
    headers: {
      Authorization: "Bearer browser-token",
      Cookie: "a=b",
      "X-Forwarded-Host": "evil",
      Accept: "application/json",
    },
  });
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { ok: true, url: "/api/v1/tasks/abc?limit=5" });
  assert.equal(response.headers.get("etag"), '"v1"');
  assert.equal(response.headers.get("set-cookie"), null);
  const request = seen.at(-1);
  assert.equal(request.headers.authorization, `Bearer ${TOKEN}`);
  assert.equal(request.headers.cookie, undefined);
  assert.equal(request.headers["x-forwarded-host"], undefined);
  assert.equal(request.headers.accept, "application/json");
});

test("relay forwards JSON bodies for unsafe methods", async () => {
  const base = await gateway();
  const response = await fetch(`${base}/api/tasks`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: base },
    body: '{"title":"x"}',
  });
  assert.equal(response.status, 200);
  assert.equal(seen.at(-1).method, "POST");
  assert.equal(seen.at(-1).body, '{"title":"x"}');
  assert.equal(seen.at(-1).headers["content-type"], "application/json");
});

test("gateway routes win over the relay and the session gate stays in front", async () => {
  const base = await gateway({ passwordFile });
  const before = seen.length;
  const session = await fetch(`${base}/api/session`);
  assert.deepEqual(await session.json(), { authenticated: false, authRequired: true });
  assert.equal((await fetch(`${base}/api/tasks`)).status, 401);
  assert.equal(seen.length, before, "unauthenticated requests reached the daemon");
});

test("relay rejects traversal and arbitrary URLs", async () => {
  const base = await gateway();
  const before = seen.length;
  for (const route of ["/api/%2e%2e/secret", "/api/a%2fb", "/api/a%5cb", "/api/a%00", "/api/", "/api//x"]) {
    const response = await raw(base, route);
    assert.equal(response.status, 400, route);
    assert.equal(response.headers["x-celeris-web-error"], "invalid_path");
  }
  assert.equal(seen.length, before);
  assert.equal(upstreamPath("/tasks/a-1"), "/api/v1/tasks/a-1");
  assert.equal(upstreamPath("/../x"), null);
  assert.throws(() => parseUpstream("http://u:p@127.0.0.1:1"));
  assert.throws(() => parseUpstream("http://127.0.0.1:1/api"));
  assert.throws(() => parseUpstream("file:///etc/passwd"));
});

test("errors distinguish daemon auth, unreachable, timeout and pass daemon errors through", async () => {
  const wrong = path.join(dir, "wrong-token");
  writeFileSync(wrong, "wrong\n");
  const unauthorized = await fetch(`${await gateway({ daemonTokenFile: wrong })}/api/tasks`);
  assert.equal(unauthorized.status, 502);
  assert.deepEqual(await unauthorized.json(), { error: "daemon_auth" });

  const closed = http.createServer();
  const closedUrl = await listen(closed);
  await new Promise((resolve) => closed.close(resolve));
  servers.splice(servers.indexOf(closed), 1);
  const unreachable = await fetch(`${await gateway({ daemonUrl: closedUrl })}/api/tasks`);
  assert.equal(unreachable.status, 502);
  assert.deepEqual(await unreachable.json(), { error: "daemon_unreachable" });

  const timeout = await fetch(`${await gateway({ relayTimeoutMs: 100 })}/api/slow`);
  assert.equal(timeout.status, 504);
  assert.deepEqual(await timeout.json(), { error: "daemon_timeout" });

  const base = await gateway();
  const missing = await fetch(`${base}/api/missing`);
  assert.equal(missing.status, 404);
  assert.equal(missing.headers.get("x-celeris-web-error"), null);

  const leak = await fetch(`${base}/api/leak`);
  assert.equal(leak.status, 500);
  const text = await leak.text();
  assert.ok(!text.includes(TOKEN), text);
  assert.ok(text.includes("[redacted]"));
  assert.equal(leak.headers.get("x-secret"), null);
});

test("browser disconnect aborts the upstream request", async () => {
  const base = await gateway();
  const start = seen.length;
  const controller = new AbortController();
  const pending = fetch(`${base}/api/slow`, { signal: controller.signal }).catch(() => null);
  const deadline = Date.now() + 5_000;
  while (!seen.slice(start).some((r) => r.url === "/api/v1/slow")) {
    if (Date.now() > deadline) throw new Error("upstream never saw the request");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  const before = aborted;
  controller.abort();
  await pending;
  while (aborted === before) {
    if (Date.now() > deadline) throw new Error("upstream was not aborted");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
});

test("token does not appear in HTML or gateway errors", async () => {
  const base = await gateway();
  for (const route of ["/", "/tasks", "/api/leak", "/api/%2e%2e/x", "/nope.js"]) {
    const { body } = await raw(base, route);
    assert.ok(!body.includes(TOKEN), route);
  }
});
