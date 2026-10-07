import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect } from "node:net";
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
  server.on("upgrade", app.locals.browserLiveUpgrade);
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  base = `http://127.0.0.1:${server.address().port}`;
});

test("rejected upgrade RST on invalid host leaves gateway serving", async () => {
  const uncaught = [];
  const onUncaught = (error) => uncaught.push(error);
  process.on("uncaughtExceptionMonitor", onUncaught);
  try {
    for (const resetOnResponse of [false, true]) {
      const socket = connect(server.address().port, "127.0.0.1");
      socket.on("error", () => {});
      const closed = new Promise((resolve) => socket.once("close", resolve));
      if (resetOnResponse) socket.once("data", () => socket.resetAndDestroy());
      else server.prependOnceListener("upgrade", () => socket.resetAndDestroy());
      await new Promise((resolve) => socket.once("connect", resolve));
      socket.write(
        "GET /api/session/1234/stream HTTP/1.1\r\nHost: evil.example\r\nConnection: Upgrade\r\n" +
          "Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n" +
          "Sec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\n\r\nextra bytes",
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
  assert.match(html.headers.get("content-security-policy"), /img-src 'self' data: blob:;/);
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

test("release path with a dotfile dir: / and SPA routes are 200, dotfiles under dist stay hidden", async () => {
  const releaseHome = mkdtempSync(path.join(tmpdir(), "celeris-web-release-"));
  const releaseDistDir = path.join(releaseHome, ".local/celeris/releases/0123456789ab/web/app/dist");
  mkdirSync(path.join(releaseDistDir, "assets"), { recursive: true });
  writeFileSync(path.join(releaseDistDir, "index.html"), "<!doctype html><title>release shell</title>");
  writeFileSync(path.join(releaseDistDir, "assets/x.js"), "export {};\n");
  writeFileSync(path.join(releaseDistDir, "assets/.secret"), "top secret asset");
  writeFileSync(path.join(releaseDistDir, ".env"), "SECRET=top secret env");

  const releaseApp = createApp({ distDir: releaseDistDir, release: "release-path-test", log: () => {} });
  const releaseServer = releaseApp.listen(0, "127.0.0.1");
  try {
    await new Promise((resolve, reject) => {
      releaseServer.once("listening", resolve);
      releaseServer.once("error", reject);
    });
    const releaseBase = `http://127.0.0.1:${releaseServer.address().port}`;
    const releaseGet = (route) => fetch(`${releaseBase}${route}`);

    for (const route of ["/", "/login", "/inbox"]) {
      const res = await releaseGet(route);
      assert.equal(res.status, 200, route);
      assert.match(await res.text(), /release shell/);
    }

    const asset = await releaseGet("/assets/x.js");
    assert.equal(asset.status, 200);

    const secretAsset = await releaseGet("/assets/.secret");
    assert.equal(secretAsset.status, 404);
    assert.doesNotMatch(await secretAsset.text(), /top secret asset/);

    const envFile = await releaseGet("/.env");
    assert.equal(envFile.status, 404);
    assert.doesNotMatch(await envFile.text(), /top secret env/);
  } finally {
    await new Promise((resolve) => releaseServer.close(resolve));
    rmSync(releaseHome, { recursive: true, force: true });
  }
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
