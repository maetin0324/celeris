import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp, parseBind, validateConfig } from "./app.js";

const distDir = mkdtempSync(path.join(tmpdir(), "celeris-web-gateway-"));
mkdirSync(path.join(distDir, "assets"));
writeFileSync(path.join(distDir, "index.html"), '<!doctype html><script src="/assets/main-abcdef012345.js"></script>');
writeFileSync(path.join(distDir, "assets/main-abcdef012345.js"), "export {};\n");
writeFileSync(path.join(distDir, "assets/plain.js"), "export {};\n");
const entries = [];
let server;
let base;

before(async () => {
  const app = createApp({ distDir, release: "test", log: (entry) => entries.push(entry) });
  server = app.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  base = `http://127.0.0.1:${server.address().port}`;
});
after(async () => {
  await new Promise((resolve) => server.close(resolve));
  rmSync(distDir, { recursive: true, force: true });
});

function get(route, options = {}) {
  return fetch(`${base}${route}`, options);
}

test("bind and non-loopback password gate", () => {
  assert.deepEqual(parseBind(), { host: "127.0.0.1", port: 7720 });
  assert.deepEqual(parseBind("[::1]:1234"), { host: "::1", port: 1234 });
  assert.throws(() => parseBind("127.0.0.1:0"));
  assert.throws(() => validateConfig({ bind: parseBind("0.0.0.0:7720") }), /PASSWORD_FILE/);
  assert.doesNotThrow(() => validateConfig({ bind: parseBind("127.0.0.1:7720") }));
  const passwordFile = path.join(distDir, "password");
  writeFileSync(passwordFile, "secret\n");
  assert.doesNotThrow(() => validateConfig({ bind: parseBind("0.0.0.0:7720"), passwordFile }));
  writeFileSync(passwordFile, "  \n");
  assert.throws(() => validateConfig({ bind: parseBind("0.0.0.0:7720"), passwordFile }), /empty/);
});

test("HTML and health are local even when no daemon exists", async () => {
  const html = await get("/tasks/abc");
  assert.equal(html.status, 200);
  assert.match(await html.text(), /main-abcdef012345/);
  assert.equal(html.headers.get("cache-control"), "no-store");
  assert.match(html.headers.get("content-security-policy"), /script-src 'self'/);
  assert.doesNotMatch(html.headers.get("content-security-policy"), /unsafe-inline/);
  const health = await get("/healthz");
  assert.deepEqual(await health.json(), { ok: true, name: "celeris-web", version: "0.1.0", release: "test" });
});

test("Host rejection precedes every route", async () => {
  for (const route of [
    "/",
    "/healthz",
    "/assets/main-abcdef012345.js",
    "/api/nope",
    "/files/nope",
    "/events",
    "/nope",
  ]) {
    const result = await new Promise((resolve, reject) => {
      const req = http.get(
        { hostname: "127.0.0.1", port: server.address().port, path: route, headers: { Host: "evil.example" } },
        resolve,
      );
      req.once("error", reject);
    });
    assert.equal(result.statusCode, 400, route);
    assert.equal(result.headers["x-frame-options"], "DENY");
    result.resume();
  }
});

test("CSRF rejects foreign origin and same-site, allows headerless CLI", async () => {
  for (const headers of [
    { Origin: "http://evil.example" },
    { Origin: "http://127.0.0.1:9999" },
    { "Sec-Fetch-Site": "same-site" },
    { "Sec-Fetch-Site": "cross-site" },
    { Origin: base, "Sec-Fetch-Site": "same-site" },
  ])
    assert.equal((await get("/api/nope", { method: "POST", headers })).status, 403);
  for (const headers of [{}, { Origin: base, "Sec-Fetch-Site": "same-origin" }, { "Sec-Fetch-Site": "none" }])
    assert.equal((await get("/api/nope", { method: "POST", headers })).status, 404);
});

test("only fingerprint assets cache; reserved and unknown assets never fall back", async () => {
  const hash = await get("/assets/main-abcdef012345.js");
  assert.match(hash.headers.get("cache-control"), /immutable/);
  const plain = await get("/assets/plain.js");
  assert.equal(plain.headers.get("cache-control"), "no-store");
  for (const route of ["/assets/missing.js", "/api/missing", "/files/nope", "/events", "/favicon.ico"])
    assert.equal((await get(route)).status, 404, route);
  assert.ok(entries.every((entry) => Object.keys(entry).sort().join(",") === "ms,path,status"));
});
