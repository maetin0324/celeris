import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { streamParams } from "./events.js";

// 偽 daemon と gateway は loopback の空き port。
const TOKEN = "fixture-daemon-token-3f9c2a71";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-events-"));
const tokenFile = path.join(dir, "token");
const passwordFile = path.join(dir, "password");
writeFileSync(tokenFile, `${TOKEN}\n`);
writeFileSync(passwordFile, "pw\n");

const seen = [];
const open = new Set();
let aborted = 0;
let status = 200;
const daemon = http.createServer((req, res) => {
  seen.push({ url: req.url, headers: req.headers });
  if (req.headers.authorization !== `Bearer ${TOKEN}`) {
    res.writeHead(401, { "content-type": "application/json" });
    return res.end('{"error":"unauthorized"}');
  }
  if (status !== 200) {
    res.writeHead(status, { "content-type": "application/json" });
    return res.end('{"error":"too_many_streams"}');
  }
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  res.write("event: hello\ndata: {}\n\n");
  open.add(res);
  res.on("close", () => {
    open.delete(res);
    if (!res.writableFinished) aborted += 1;
  });
});

const servers = [];
let upstream;
let base;

async function listen(server) {
  server.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  servers.push(server);
  return `http://127.0.0.1:${server.address().port}`;
}

before(async () => {
  upstream = await listen(daemon);
  // JSON の timeout を短くしても SSE には掛からないことを確かめる。
  base = await listen(
    http.createServer(
      createApp({ daemonUrl: upstream, daemonTokenFile: tokenFile, relayTimeoutMs: 100, log: () => {} }),
    ),
  );
});

after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

async function readUntil(reader, text) {
  const decoder = new TextDecoder();
  let got = "";
  while (!got.includes(text)) {
    const { value, done } = await reader.read();
    if (done) break;
    got += decoder.decode(value, { stream: true });
  }
  return got;
}

test("streamParams accepts event ids and task ids only", () => {
  assert.equal(String(streamParams("after_id=5&task_id=01ABC", "7").query), "after_id=5&task_id=01ABC");
  assert.equal(streamParams("", " 7 ").lastEventId, "7");
  for (const [search, id] of [
    ["after_id=-1", undefined],
    ["after_id=x", undefined],
    ["task_id=a%2Fb", undefined],
    ["task_id=../x", undefined],
    ["types=x", undefined],
    ["after_id=1&after_id=2", undefined],
    ["", "abc"],
    ["", "1\n2"],
  ])
    assert.equal(streamParams(search, id), null, `${search} ${id}`);
});

test("stream is relayed with event-stream headers and forwarded cursor, past the JSON timeout", async () => {
  const controller = new AbortController();
  const res = await fetch(`${base}/events?task_id=T1&after_id=3`, {
    headers: { "Last-Event-ID": "9", Authorization: "Bearer browser" },
    signal: controller.signal,
  });
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("content-type"), "text/event-stream");
  assert.equal(res.headers.get("cache-control"), "no-store");
  assert.equal(res.headers.get("x-accel-buffering"), "no");
  const req = seen.at(-1);
  assert.equal(req.url, "/api/v1/stream?task_id=T1&after_id=3");
  assert.equal(req.headers["last-event-id"], "9");
  assert.equal(req.headers.authorization, `Bearer ${TOKEN}`);
  const reader = res.body.getReader();
  assert.match(await readUntil(reader, "hello"), /event: hello/);
  await new Promise((r) => setTimeout(r, 400));
  for (const client of open) client.write("event: heartbeat\ndata: {}\n\n");
  assert.match(await readUntil(reader, "heartbeat"), /event: heartbeat/);
  const before = aborted;
  controller.abort();
  for (let i = 0; i < 50 && aborted === before; i += 1) await new Promise((r) => setTimeout(r, 20));
  assert.equal(aborted, before + 1);
});

test("invalid cursor or query is 400 before reaching the daemon", async () => {
  const count = seen.length;
  for (const [query, id] of [
    ["?after_id=x", undefined],
    ["?task_id=a/b", undefined],
    ["?other=1", undefined],
    ["", "nope"],
  ]) {
    const res = await fetch(`${base}/events${query}`, { headers: id ? { "Last-Event-ID": id } : {} });
    assert.equal(res.status, 400, `${query} ${id}`);
    await res.text();
  }
  assert.equal(seen.length, count);
});

test("daemon 503 and 401 are passed through", async () => {
  status = 503;
  const busy = await fetch(`${base}/events`);
  assert.equal(busy.status, 503);
  assert.match(await busy.text(), /too_many_streams/);
  status = 200;
  const noToken = await listen(http.createServer(createApp({ daemonUrl: upstream, log: () => {} })));
  const denied = await fetch(`${noToken}/events`);
  assert.equal(denied.status, 401);
  assert.equal(denied.headers.get("x-celeris-web-error"), null);
  assert.match(await denied.text(), /unauthorized/);
});

test("unauthenticated /events is 401 when a password is set", async () => {
  const locked = await listen(
    http.createServer(createApp({ daemonUrl: upstream, daemonTokenFile: tokenFile, passwordFile, log: () => {} })),
  );
  const count = seen.length;
  const res = await fetch(`${locked}/events`);
  assert.equal(res.status, 401);
  await res.text();
  assert.equal(seen.length, count);
});
