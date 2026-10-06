import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { dispositionFor, filenameOf, filePath, fileQuery, viewCspFor, viewDisposition, viewTypeFor } from "./files.js";

// 偽 daemon と gateway は loopback の空き port。token は fixture で、どの出力にも出てはいけない。
const TOKEN = "fixture-daemon-token-3f9c2a71";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-files-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${TOKEN}\n`);
const BODY = Buffer.from("0123456789abcdefghij");

const seen = [];
let aborted = 0;
const daemon = http.createServer((req, res) => {
  seen.push({ url: req.url, headers: req.headers });
  if (req.headers.authorization !== `Bearer ${TOKEN}`) {
    res.writeHead(401, { "content-type": "application/json" });
    return res.end('{"error":"unauthorized"}');
  }
  const url = new URL(req.url, "http://x");
  if (url.pathname === "/api/v1/tasks/T1/runs/R1/slow") {
    res.writeHead(200, { "content-type": "application/octet-stream" });
    res.write("first");
    res.on("close", () => {
      if (!res.writableFinished) aborted += 1;
    });
    return;
  }
  if (url.pathname === "/api/v1/tasks/T1/artifacts/1") {
    res.writeHead(200, {
      "content-type": "text/html; charset=utf-8",
      "content-disposition": 'inline; filename="report.html"',
      "content-security-policy": "default-src *",
      "set-cookie": "daemon=1",
    });
    return res.end("<script>alert(1)</script>");
  }
  // 実 daemon と同じく、表に無い拡張子（html・svg・pdf）は octet-stream と inline の filename で返す。
  const octet = { 3: "report.html", 4: "paper.pdf", 5: "figure.svg" }[url.pathname.split("/").at(-1)];
  if (url.pathname.startsWith("/api/v1/tasks/T1/artifacts/") && octet) {
    res.writeHead(200, {
      "content-type": "application/octet-stream",
      "content-disposition": `${url.searchParams.has("download") ? "attachment" : "inline"}; filename="${octet}"; filename*=UTF-8''${octet}`,
    });
    return res.end(`<script>parent.document.title='ran'</script>${url.search}`);
  }
  if (url.pathname === "/api/v1/tasks/T1/artifacts/2") {
    res.writeHead(200, { "content-type": "image/svg+xml" });
    return res.end("<svg/>");
  }
  const range = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? "");
  if (range) {
    const [start, end] = [Number(range[1]), Number(range[2])];
    if (start >= BODY.length) {
      res.writeHead(416, { "content-range": `bytes */${BODY.length}` });
      return res.end();
    }
    res.writeHead(206, {
      "content-type": "text/plain",
      "content-range": `bytes ${start}-${end}/${BODY.length}`,
      "accept-ranges": "bytes",
      "content-length": end - start + 1,
    });
    return res.end(BODY.subarray(start, end + 1));
  }
  res.writeHead(200, {
    "content-type": "text/plain",
    "accept-ranges": "bytes",
    "x-celeris-size": String(BODY.length),
    "x-daemon-internal": "1",
  });
  res.end(JSON.stringify({ path: url.pathname, query: url.search }));
});

const servers = [];
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
  const upstream = await listen(daemon);
  base = await listen(http.createServer(createApp({ daemonUrl: upstream, daemonTokenFile: tokenFile, log: () => {} })));
});

after(async () => {
  for (const server of servers) {
    server.closeAllConnections?.();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

test("filePath accepts runs and artifacts, rejects traversal, separators and NUL", () => {
  assert.equal(filePath("/tasks/T1/runs/R1/stdout"), "/api/v1/tasks/T1/runs/R1/stdout");
  assert.equal(filePath("/tasks/T1/artifacts/3"), "/api/v1/tasks/T1/artifacts/3");
  for (const bad of [
    "/tasks/T1/runs/R1/..",
    "/tasks/T1/runs/R1/%2e%2e",
    "/tasks/T1/runs/R1/a%2Fb",
    "/tasks/T1/runs/R1/a%5Cb",
    "/tasks/T1/runs/R1/a%00b",
    "/tasks/T1/runs/R1/a/b",
    "/tasks/../runs/R1/x",
    "/tasks/T1/artifacts/-1",
    "/tasks/T1/artifacts/1.5",
    "/tasks/T1/artifacts/%31%2F",
    "/tasks/T1/other/x",
    "/tasks/T1/runs/R1/%zz",
  ])
    assert.equal(filePath(bad), null, bad);
});

test("fileQuery keeps offset, length and download=1 only", () => {
  assert.equal(String(fileQuery("offset=10&length=5&download=1")), "offset=10&length=5&download=1");
  for (const bad of ["offset=-1", "length=abc", "download=0", "url=http://x", "offset=1&offset=2"])
    assert.equal(fileQuery(bad), null, bad);
});

test("dispositionFor forces attachment for active content only", () => {
  assert.equal(dispositionFor("text/html", null), "attachment");
  assert.equal(dispositionFor("text/html", 'inline; filename="a.html"'), 'attachment; filename="a.html"');
  assert.equal(dispositionFor("application/xhtml+xml", null), "attachment");
  assert.equal(dispositionFor("image/svg+xml", null), "attachment");
  assert.equal(dispositionFor("text/plain", null), null);
  assert.equal(dispositionFor("application/json", "inline"), "inline");
});

test("fileQuery accepts view=1 but not together with download", () => {
  assert.equal(String(fileQuery("view=1")), "view=1");
  assert.equal(String(fileQuery("offset=0&length=10&view=1")), "offset=0&length=10&view=1");
  for (const bad of ["view=0", "view=1&download=1", "view=1&view=1"]) assert.equal(fileQuery(bad), null, bad);
});

test("view helpers fill html/svg/pdf types by name and keep sandbox except for pdf", () => {
  const daemon = (name) => `inline; filename="${name}"; filename*=UTF-8''${encodeURIComponent(name)}`;
  assert.equal(filenameOf(daemon("報告.html")), "報告.html");
  assert.equal(filenameOf('attachment; filename="a.pdf"'), "a.pdf");
  assert.equal(filenameOf(null), "");
  assert.equal(viewTypeFor("application/octet-stream", daemon("a.HTML")), "text/html; charset=utf-8");
  assert.equal(viewTypeFor("application/octet-stream", daemon("a.htm")), "text/html; charset=utf-8");
  assert.equal(viewTypeFor("application/octet-stream", daemon("a.svg")), "image/svg+xml");
  assert.equal(viewTypeFor(null, daemon("a.pdf")), "application/pdf");
  assert.equal(viewTypeFor("application/octet-stream", daemon("a.zip")), "application/octet-stream");
  // daemon が種類を決めた応答は拡張子で上書きしない（text/plain の .html 名でも HTML にしない）。
  assert.equal(viewTypeFor("text/plain; charset=utf-8", daemon("a.html")), "text/plain; charset=utf-8");
  assert.equal(viewDisposition(daemon("a.html")), daemon("a.html"));
  assert.equal(viewDisposition('attachment; filename="a.svg"'), 'inline; filename="a.svg"');
  assert.equal(viewDisposition(null), "inline");
  for (const type of ["text/html; charset=utf-8", "image/svg+xml", "text/plain", "application/xml"]) {
    const csp = viewCspFor(type);
    assert.match(csp, /^sandbox;/, type);
    assert.doesNotMatch(csp, /allow-scripts|allow-same-origin|script-src/, type);
    assert.match(csp, /frame-ancestors 'self'/, type);
  }
  const pdf = viewCspFor("application/pdf");
  assert.doesNotMatch(pdf, /sandbox|script-src|unsafe/);
  assert.match(pdf, /^default-src 'none';/);
  assert.match(pdf, /frame-ancestors 'self'/);
});

test("run file relays offset/length/download and allowlisted headers with nosniff", async () => {
  const res = await fetch(`${base}/files/tasks/T1/runs/R1/stdout?offset=10&length=5&download=1`, {
    headers: { Authorization: "Bearer browser", Cookie: "a=b" },
  });
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), {
    path: "/api/v1/tasks/T1/runs/R1/stdout",
    query: "?offset=10&length=5&download=1",
  });
  const req = seen.at(-1);
  assert.equal(req.headers.authorization, `Bearer ${TOKEN}`);
  assert.equal(req.headers.cookie, undefined);
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("x-celeris-size"), "20");
  assert.equal(res.headers.get("accept-ranges"), "bytes");
  assert.equal(res.headers.get("x-daemon-internal"), null);
  assert.match(res.headers.get("content-security-policy") ?? "", /sandbox/);
});

test("Range is forwarded and 206 / 416 with Content-Range are kept", async () => {
  const partial = await fetch(`${base}/files/tasks/T1/runs/R1/stdout`, { headers: { Range: "bytes=2-5" } });
  assert.equal(partial.status, 206);
  assert.equal(partial.headers.get("content-range"), "bytes 2-5/20");
  assert.equal(await partial.text(), "2345");
  const outside = await fetch(`${base}/files/tasks/T1/runs/R1/stdout`, { headers: { Range: "bytes=99-100" } });
  assert.equal(outside.status, 416);
  assert.equal(outside.headers.get("content-range"), "bytes */20");
  const bad = await fetch(`${base}/files/tasks/T1/runs/R1/stdout`, { headers: { Range: "lines=1-2" } });
  assert.equal(bad.status, 400);
});

test("HTML and SVG artifacts are downloaded, never rendered same-origin", async () => {
  const html = await fetch(`${base}/files/tasks/T1/artifacts/1`);
  assert.equal(html.status, 200);
  assert.equal(html.headers.get("content-disposition"), 'attachment; filename="report.html"');
  assert.equal(html.headers.get("x-content-type-options"), "nosniff");
  assert.match(html.headers.get("content-security-policy") ?? "", /^sandbox/);
  assert.equal(html.headers.get("set-cookie"), null);
  await html.text();
  const svg = await fetch(`${base}/files/tasks/T1/artifacts/2`);
  assert.equal(svg.headers.get("content-disposition"), "attachment");
  await svg.text();
});

test("view=1 shows HTML/SVG inline only under CSP sandbox, never forwarding view to the daemon", async () => {
  for (const [idx, type] of [
    [3, "text/html; charset=utf-8"],
    [5, "image/svg+xml"],
  ]) {
    const res = await fetch(`${base}/files/tasks/T1/artifacts/${idx}?view=1`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("content-type"), type);
    assert.match(res.headers.get("content-disposition") ?? "", /^inline; filename="(report\.html|figure\.svg)"/);
    assert.equal(res.headers.get("x-content-type-options"), "nosniff");
    const csp = res.headers.get("content-security-policy") ?? "";
    assert.match(csp, /^sandbox; default-src 'none';/);
    assert.doesNotMatch(csp, /allow-scripts|allow-same-origin|script-src/);
    assert.match(csp, /frame-ancestors 'self'$/);
    assert.equal(res.headers.get("x-frame-options"), "SAMEORIGIN");
    // daemon には view を送らない（daemon の query は offset・length・download だけ）。
    assert.equal(await res.text(), "<script>parent.document.title='ran'</script>");
    assert.equal(new URL(seen.at(-1).url, "http://x").search, "");
  }
  // daemon が text/html と言った応答も、view=1 なら sandbox 付きの inline。
  const declared = await fetch(`${base}/files/tasks/T1/artifacts/1?view=1`);
  assert.equal(declared.headers.get("content-disposition"), 'inline; filename="report.html"');
  assert.match(declared.headers.get("content-security-policy") ?? "", /^sandbox;/);
  assert.equal(declared.headers.get("set-cookie"), null);
  await declared.text();
});

test("view=1 PDF is inline application/pdf with default-src 'none' and self-only framing", async () => {
  const res = await fetch(`${base}/files/tasks/T1/artifacts/4?view=1`);
  assert.equal(res.headers.get("content-type"), "application/pdf");
  assert.match(res.headers.get("content-disposition") ?? "", /^inline; filename="paper\.pdf"/);
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(
    res.headers.get("content-security-policy"),
    "default-src 'none'; object-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'",
  );
  await res.text();
});

test("without view, HTML/SVG/PDF named octet-stream stay octet-stream under frame-ancestors 'none'", async () => {
  for (const idx of [3, 4, 5]) {
    const res = await fetch(`${base}/files/tasks/T1/artifacts/${idx}?download=1`);
    assert.equal(res.headers.get("content-type"), "application/octet-stream");
    assert.match(res.headers.get("content-disposition") ?? "", /^attachment;/);
    assert.equal(res.headers.get("content-security-policy"), "sandbox; default-src 'none'; frame-ancestors 'none'");
    assert.equal(res.headers.get("x-frame-options"), "DENY");
    await res.text();
  }
  const bad = await fetch(`${base}/files/tasks/T1/artifacts/3?view=1&download=1`);
  assert.equal(bad.status, 400);
  await bad.text();
});

test("invalid name, idx and query are rejected before reaching the daemon", async () => {
  const count = seen.length;
  for (const bad of [
    "/files/tasks/T1/runs/R1/%2e%2e",
    "/files/tasks/T1/runs/R1/a%2Fb",
    "/files/tasks/T1/runs/R1/a%00b",
    "/files/tasks/T1/artifacts/x",
    "/files/tasks/T1/artifacts/1?url=http://evil",
  ]) {
    const res = await fetch(`${base}${bad}`);
    assert.equal(res.status, 400, bad);
    await res.text();
  }
  assert.equal(seen.length, count);
});

test("browser disconnect aborts the upstream body without buffering", async () => {
  const controller = new AbortController();
  const res = await fetch(`${base}/files/tasks/T1/runs/R1/slow`, { signal: controller.signal });
  assert.equal(res.status, 200);
  const reader = res.body.getReader();
  const { value } = await reader.read();
  assert.equal(Buffer.from(value).toString(), "first");
  controller.abort();
  for (let i = 0; i < 50 && aborted === 0; i += 1) await new Promise((r) => setTimeout(r, 20));
  assert.equal(aborted, 1);
});

test("daemon 401 is reported as daemon_auth, not a session 401", async () => {
  const other = await listen(
    http.createServer(createApp({ daemonUrl: base.replace(/:\d+$/, `:${daemon.address().port}`), log: () => {} })),
  );
  const res = await fetch(`${other}/files/tasks/T1/runs/R1/stdout`);
  assert.equal(res.status, 502);
  assert.equal(res.headers.get("x-celeris-web-error"), "daemon_auth");
  assert.ok(!(await res.text()).includes(TOKEN));
});
