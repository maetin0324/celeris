import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { consoleParams } from "./console.js";

const TOKEN = "fixture-daemon-token-3f9c2a71";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-console-"));
const tokenFile = path.join(dir, "token");
const passwordFile = path.join(dir, "password");
writeFileSync(tokenFile, `${TOKEN}\n`);
writeFileSync(passwordFile, "pw\n");
const seen = [];
const open = new Set();
const daemon = http.createServer((req, res) => {
  seen.push({ url: req.url, headers: req.headers });
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.write('event: hello\ndata: {"cursor":"c1"}\n\n');
  open.add(res);
  res.on("close", () => open.delete(res));
});
const servers = [];
let base;
let authed;
async function listen(server) {
  server.listen(0, "127.0.0.1");
  await new Promise((resolve) => server.once("listening", resolve));
  servers.push(server);
  return `http://127.0.0.1:${server.address().port}`;
}
before(async () => {
  const upstream = await listen(daemon);
  base = await listen(http.createServer(createApp({ daemonUrl: upstream, daemonTokenFile: tokenFile, log: () => {} })));
  authed = await listen(
    http.createServer(createApp({ daemonUrl: upstream, daemonTokenFile: tokenFile, passwordFile, log: () => {} })),
  );
});
after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

test("consoleParams keeps scope and since only", () => {
  assert.equal(String(consoleParams("scope=node:cos&since=abc.1")), "scope=node%3Acos&since=abc.1");
  for (const bad of ["scope=x", "scope=all&scope=all", "since=a%2Fb", "limit=3", "scope=node:"])
    assert.equal(consoleParams(bad), null, bad);
});

test("stream is relayed to /api/v1/console/stream with scope and since", async () => {
  const controller = new AbortController();
  const res = await fetch(`${base}/console/stream?scope=project%3AP1&since=cur9`, { signal: controller.signal });
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "text/event-stream");
  const req = seen.at(-1);
  assert.equal(req.url, "/api/v1/console/stream?scope=project%3AP1&since=cur9");
  assert.equal(req.headers.authorization, `Bearer ${TOKEN}`);
  controller.abort();
  assert.equal((await fetch(`${base}/console/stream?scope=bad`)).status, 400);
});

test("unauthenticated stream is 401, not a redirect", async () => {
  const res = await fetch(`${authed}/console/stream?scope=all`, { redirect: "manual" });
  assert.equal(res.status, 401);
});
