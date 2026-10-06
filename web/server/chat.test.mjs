import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { chatRoute, chatStreamParams, DEFAULT_CHAT_UPLOAD_LIMIT_BYTES, parseUploadLimit } from "./chat.js";

// 偽 daemon と gateway は loopback の空き port。待ちは sleep でなく出来事（daemon が受けた・閉じた）で行う。
const TOKEN = "fixture-daemon-token-c0a7e1d2";
const LIMIT = 1024;
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-chat-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${TOKEN}\n`);
writeFileSync(path.join(dir, "index.html"), "<!doctype html><title>celeris</title>");

// daemon 側の出来事を test から待つ。name ごとに最初の 1 件を resolve する。
const waiters = new Map();
function signal(name, value) {
  const entry = waiters.get(name) ?? deferred();
  waiters.set(name, entry);
  entry.resolve(value);
}
function waitFor(name) {
  const entry = waiters.get(name) ?? deferred();
  waiters.set(name, entry);
  return entry.promise;
}
function deferred() {
  let resolve;
  const promise = new Promise((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const seen = [];
const daemon = http.createServer((req, res) => {
  const record = { method: req.method, url: req.url, headers: req.headers, bytes: 0, complete: false };
  seen.push(record);
  if (req.headers.authorization !== `Bearer ${TOKEN}`) {
    res.writeHead(401, { "content-type": "application/json" });
    return res.end('{"error":"unauthorized"}');
  }
  const name = new URL(req.url, "http://x").pathname.split("/").at(-2);
  if (req.url.endsWith("/attachments")) {
    const chunks = [];
    req.on("data", (chunk) => {
      chunks.push(chunk);
      record.bytes += chunk.length;
      signal(`upload-data:${name}`, record);
    });
    req.on("end", () => {
      record.complete = true;
      record.body = Buffer.concat(chunks);
      res.writeHead(201, { "content-type": "application/json" });
      res.end(JSON.stringify({ attachment: { id: "a1", size_bytes: record.bytes } }));
    });
    req.on("close", () => signal(`upload-closed:${name}`, record));
    return;
  }
  if (req.url.startsWith("/api/v1/chat/threads/expired/stream")) {
    res.writeHead(410, { "content-type": "application/problem+json" });
    return res.end('{"type":"about:blank","title":"Gone","status":410,"code":"chat-cursor-expired"}');
  }
  if (req.url.startsWith("/api/v1/chat/threads/t1/stream")) {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.write(': hello\n\nid: 5\nevent: text_delta\ndata: {"id":"5","type":"text_delta"}\n\n');
    signal("stream-open", { req, res });
    res.on("close", () => signal("stream-closed", res.writableFinished));
    return;
  }
  if (req.url === "/api/v1/chat/attachments/a1/content") {
    res.writeHead(200, {
      "content-type": "application/pdf",
      "content-disposition": "attachment; filename=\"r.pdf\"; filename*=UTF-8''%E5%A0%B1%E5%91%8A.pdf",
      "content-length": "8",
      "x-internal": "drop-me",
    });
    return res.end(req.method === "HEAD" ? undefined : "%PDF-1.7");
  }
  if (req.url === "/api/v1/chat/attachments/bare/content") {
    res.writeHead(200, { "content-type": "image/svg+xml" });
    return res.end("<svg/>");
  }
  if (req.url === "/api/v1/chat/attachments/a1/preview") {
    res.writeHead(200, { "content-type": "image/png", "content-disposition": "inline" });
    return res.end(Buffer.from([0x89, 0x50, 0x4e, 0x47]));
  }
  if (req.url === "/api/v1/chat/attachments/nopreview/preview") {
    res.writeHead(404, { "content-type": "application/problem+json" });
    return res.end('{"title":"Not Found","status":404}');
  }
  req.resume();
  req.on("end", () => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ relayed: req.url }));
  });
});

const servers = [];
let upstream;
let base;

async function listen(server) {
  if (!server.listening) server.listen(0, "127.0.0.1");
  if (!server.listening) await once(server, "listening");
  servers.push(server);
  return `http://127.0.0.1:${server.address().port}`;
}

async function gateway(options = {}) {
  const app = createApp({
    distDir: dir,
    daemonUrl: upstream,
    daemonTokenFile: tokenFile,
    chatUploadLimitBytes: LIMIT,
    log: () => {},
    ...options,
  });
  return listen(app.listen(0, "127.0.0.1"));
}

// 本文を chunked で送る生の request（Content-Length を付けない）。応答は promise で返す。
function openUpload(thread, headers = {}) {
  const { hostname, port } = new URL(base);
  const req = http.request({
    hostname,
    port,
    method: "POST",
    path: `/api/chat/threads/${thread}/attachments`,
    headers: { "content-type": "multipart/form-data; boundary=xyz", cookie: "s=1", ...headers },
  });
  const response = new Promise((resolve, reject) => {
    req.on("response", (res) => {
      let body = "";
      res.on("data", (chunk) => {
        body += chunk;
      });
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body }));
    });
    req.on("error", reject);
  });
  return { req, response };
}

before(async () => {
  upstream = await listen(daemon);
  base = await gateway();
});
after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

test("route, query and limit parsing", () => {
  assert.equal(parseUploadLimit(undefined), DEFAULT_CHAT_UPLOAD_LIMIT_BYTES);
  assert.equal(DEFAULT_CHAT_UPLOAD_LIMIT_BYTES, 100 * 1024 * 1024);
  assert.equal(parseUploadLimit("2048"), 2048);
  assert.throws(() => parseUploadLimit("0"));
  assert.throws(() => parseUploadLimit("1e9"));
  assert.deepEqual(chatRoute("POST", "/chat/threads/t1/attachments"), {
    kind: "upload",
    target: "/api/v1/chat/threads/t1/attachments",
  });
  assert.equal(chatRoute("GET", "/chat/threads/t1/attachments"), null);
  assert.equal(chatRoute("POST", "/chat/threads/%2e%2e/attachments"), null);
  assert.equal(chatRoute("GET", "/chat/threads"), null);
  assert.equal(chatRoute("GET", "/chat/attachments/a1/content").kind, "content");
  assert.equal(chatRoute("HEAD", "/chat/attachments/a1/preview").kind, "preview");
  assert.deepEqual(chatStreamParams("after=12", " 12 ").lastEventId, "12");
  assert.equal(chatStreamParams("after=x"), null);
  assert.equal(chatStreamParams("after=1&after=2"), null);
  assert.equal(chatStreamParams("q=1"), null);
  assert.equal(chatStreamParams("", "abc"), null);
});

test("upload of exactly the limit streams to the daemon before the browser finishes", async () => {
  const { req, response } = openUpload("exact");
  const first = Buffer.alloc(LIMIT - 1, 0x61);
  req.write(first);
  // daemon が先頭を受けたのは、ブラウザがまだ本文を送り終えていない時点（gateway が貯めていない）。
  const record = await waitFor("upload-data:exact");
  assert.equal(record.complete, false);
  req.end(Buffer.from("b"));
  const result = await response;
  assert.equal(result.status, 201);
  assert.deepEqual(JSON.parse(result.body), { attachment: { id: "a1", size_bytes: LIMIT } });
  assert.equal(record.body.length, LIMIT);
  assert.equal(record.body.toString(), `${first}b`);
  assert.equal(record.headers.authorization, `Bearer ${TOKEN}`);
  assert.equal(record.headers["content-type"], "multipart/form-data; boundary=xyz");
  assert.equal(record.headers.cookie, undefined);
});

test("upload over the limit is 413 and aborts the upstream", async () => {
  const { req, response } = openUpload("over");
  req.on("error", () => {});
  req.write(Buffer.alloc(LIMIT, 0x61));
  const record = await waitFor("upload-data:over");
  req.write(Buffer.from("cd"));
  const result = await response;
  assert.equal(result.status, 413);
  assert.equal(result.headers["x-celeris-web-error"], "request_too_large");
  assert.deepEqual(JSON.parse(result.body), { error: "request_too_large" });
  await waitFor("upload-closed:over");
  assert.equal(record.complete, false);
  assert.ok(record.bytes <= LIMIT, `daemon received ${record.bytes} bytes`);
  req.destroy();
});

test("declared Content-Length over the limit is 413 without reaching the daemon", async () => {
  const before = seen.length;
  const { req, response } = openUpload("declared", { "content-length": String(LIMIT + 1) });
  req.on("error", () => {});
  req.flushHeaders();
  const result = await response;
  assert.equal(result.status, 413);
  assert.equal(result.headers["x-celeris-web-error"], "request_too_large");
  assert.equal(seen.length, before);
  req.destroy();
});

test("browser disconnect during upload aborts the upstream", async () => {
  const { req, response } = openUpload("gone");
  response.catch(() => {});
  req.on("error", () => {});
  req.write(Buffer.alloc(10, 0x61));
  const record = await waitFor("upload-data:gone");
  req.destroy();
  await waitFor("upload-closed:gone");
  assert.equal(record.complete, false);
});

test("chat SSE is relayed without a timeout, with Last-Event-ID, and aborted on disconnect", async () => {
  const short = await gateway({ relayTimeoutMs: 1 });
  const controller = new AbortController();
  const response = await fetch(`${short}/api/chat/threads/t1/stream?after=4`, {
    headers: { "Last-Event-ID": "4", Cookie: "s=1" },
    signal: controller.signal,
  });
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "text/event-stream");
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.equal(response.headers.get("x-accel-buffering"), "no");
  const { req: upstreamReq, res: upstreamRes } = await waitFor("stream-open");
  assert.equal(upstreamReq.url, "/api/v1/chat/threads/t1/stream?after=4");
  assert.equal(upstreamReq.headers["last-event-id"], "4");
  assert.equal(upstreamReq.headers.accept, "text/event-stream");
  assert.equal(upstreamReq.headers.cookie, undefined);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  while (!text.includes("id: 5")) text += decoder.decode((await reader.read()).value);
  // relayTimeoutMs=1 の timer は後に登録した 20ms の timer より必ず先に発火する。それでも stream は続く。
  await new Promise((resolve) => setTimeout(resolve, 20));
  upstreamRes.write('id: 6\nevent: message\ndata: {"id":"6"}\n\n');
  while (!text.includes("id: 6")) text += decoder.decode((await reader.read()).value);
  assert.match(text, /: hello\n\nid: 5\nevent: text_delta\ndata: \{"id":"5","type":"text_delta"\}\n\nid: 6/);
  controller.abort();
  assert.equal(await waitFor("stream-closed"), false);
});

test("chat SSE passes 410 problem+json through and rejects bad cursors", async () => {
  const expired = await fetch(`${base}/api/chat/threads/expired/stream?after=1`);
  assert.equal(expired.status, 410);
  assert.equal(expired.headers.get("content-type"), "application/problem+json");
  assert.deepEqual(await expired.json(), {
    type: "about:blank",
    title: "Gone",
    status: 410,
    code: "chat-cursor-expired",
  });
  const before = seen.length;
  for (const [query, lastEventId] of [
    ["after=abc", undefined],
    ["after=1&task_id=x", undefined],
    ["", "-1"],
  ]) {
    const response = await fetch(`${base}/api/chat/threads/t1/stream${query ? `?${query}` : ""}`, {
      headers: lastEventId ? { "Last-Event-ID": lastEventId } : {},
    });
    assert.equal(response.status, 400, query);
    assert.equal(response.headers.get("x-celeris-web-error"), "invalid_query");
  }
  assert.equal(seen.length, before);
});

test("attachment content and preview keep Content-Type, Content-Disposition and nosniff", async () => {
  const content = await fetch(`${base}/api/chat/attachments/a1/content`);
  assert.equal(content.status, 200);
  assert.equal(content.headers.get("content-type"), "application/pdf");
  assert.equal(
    content.headers.get("content-disposition"),
    "attachment; filename=\"r.pdf\"; filename*=UTF-8''%E5%A0%B1%E5%91%8A.pdf",
  );
  assert.equal(content.headers.get("x-content-type-options"), "nosniff");
  assert.match(content.headers.get("content-security-policy"), /^sandbox/);
  assert.equal(content.headers.get("x-internal"), null);
  assert.equal(await content.text(), "%PDF-1.7");

  const head = await fetch(`${base}/api/chat/attachments/a1/content`, { method: "HEAD" });
  assert.equal(head.status, 200);
  assert.equal(head.headers.get("content-length"), "8");
  assert.equal(seen.at(-1).method, "HEAD");

  // daemon が Disposition を付けなくても、content はダウンロード専用（SVG を同じ origin で開かせない）。
  const bare = await fetch(`${base}/api/chat/attachments/bare/content`);
  assert.equal(bare.headers.get("content-disposition"), "attachment");
  assert.equal(bare.headers.get("content-type"), "image/svg+xml");
  assert.equal(bare.headers.get("x-content-type-options"), "nosniff");

  const preview = await fetch(`${base}/api/chat/attachments/a1/preview`);
  assert.equal(preview.status, 200);
  assert.equal(preview.headers.get("content-type"), "image/png");
  assert.equal(preview.headers.get("content-disposition"), "inline");
  assert.equal(preview.headers.get("x-content-type-options"), "nosniff");
  assert.deepEqual([...new Uint8Array(await preview.arrayBuffer())], [0x89, 0x50, 0x4e, 0x47]);

  const missing = await fetch(`${base}/api/chat/attachments/nopreview/preview`);
  assert.equal(missing.status, 404);
  assert.equal(missing.headers.get("content-type"), "application/problem+json");
  assert.equal(missing.headers.get("x-celeris-web-error"), null);
});

test("other chat endpoints stay on the JSON relay", async () => {
  const list = await fetch(`${base}/api/chat/threads?limit=5`);
  assert.equal(list.status, 200);
  assert.deepEqual(await list.json(), { relayed: "/api/v1/chat/threads?limit=5" });
  const send = await fetch(`${base}/api/chat/threads/t1/messages`, {
    method: "POST",
    headers: { "content-type": "application/json", origin: base },
    body: '{"client_message_id":"c","text":"hi"}',
  });
  assert.equal(send.status, 200);
  assert.deepEqual(await send.json(), { relayed: "/api/v1/chat/threads/t1/messages" });
  // JSON relay の 1mb 上限はチャット以外では変わらない。
  const big = await fetch(`${base}/api/chat/threads/t1/messages`, {
    method: "POST",
    headers: { "content-type": "application/json", origin: base },
    body: "x".repeat(2 * 1024 * 1024),
  });
  assert.equal(big.status, 413);
});

test("daemon auth failures on uploads are 502 daemon_auth", async () => {
  const wrong = path.join(dir, "wrong");
  writeFileSync(wrong, "wrong\n");
  const other = await gateway({ daemonTokenFile: wrong });
  const response = await fetch(`${other}/api/chat/threads/x/attachments`, {
    method: "POST",
    headers: { "content-type": "multipart/form-data; boundary=xyz", origin: other },
    body: "abc",
  });
  assert.equal(response.status, 502);
  assert.equal(response.headers.get("x-celeris-web-error"), "daemon_auth");
});
