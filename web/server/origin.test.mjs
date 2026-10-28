import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp, validateConfig } from "./app.js";
import { DEVICE_COOKIE_NAME } from "./browser-live.js";
import { createOriginPolicy, isLoopbackAddress, parsePublicOrigins } from "./origin.js";

// HTTPS 前段（tailscale serve 等が TLS を終端し、平文で 127.0.0.1 の gateway へ渡す）の後ろでの自 origin の判定。
// 前段は「Host を保ったまま X-Forwarded-Proto: https を付けて loopback から接続する」要求で模す。
// ADR 2026-10-28-web-https-front-pwa D1。

const PUBLIC = "home-dev.example.ts.net";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-origin-"));
const passwordFile = path.join(dir, "password");
writeFileSync(passwordFile, "pw\n", { mode: 0o600 });
mkdirSync(path.join(dir, "dist", "icons"), { recursive: true });
writeFileSync(path.join(dir, "dist", "index.html"), "<!doctype html><title>web</title>");
writeFileSync(path.join(dir, "dist", "manifest.webmanifest"), '{"name":"Celeris"}');
writeFileSync(path.join(dir, "dist", "icons", "icon-192.png"), "png");

let proxied;
let direct;

async function listen(options) {
  const app = createApp({
    distDir: path.join(dir, "dist"),
    passwordFile,
    failedLoginDelayMs: 0,
    log: () => {},
    ...options,
  });
  const server = app.listen(0, "127.0.0.1");
  server.on("upgrade", app.locals.browserLiveUpgrade);
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  return { server, port: server.address().port };
}

before(async () => {
  proxied = await listen({ publicOrigins: `https://${PUBLIC}`, allowedHosts: "other.example.ts.net" });
  direct = await listen({});
});
after(async () => {
  for (const { server } of [proxied, direct]) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
  rmSync(dir, { recursive: true, force: true });
});

function request({ port }, method, pathname, { headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: "127.0.0.1",
        port,
        method,
        path: pathname,
        headers: { ...headers, ...(body ? { "content-type": "application/json", accept: "application/json" } : {}) },
      },
      (res) => {
        let text = "";
        res.on("data", (chunk) => {
          text += chunk;
        });
        res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, text }));
      },
    );
    req.on("error", reject);
    req.end(body ? JSON.stringify(body) : undefined);
  });
}

const viaProxy = { host: PUBLIC, origin: `https://${PUBLIC}`, "x-forwarded-proto": "https" };

async function login(target, headers) {
  const res = await request(target, "POST", "/login", { headers, body: { password: "pw" } });
  return { ...res, cookie: res.status === 200 ? res.headers["set-cookie"][0] : null };
}

// 生の upgrade を送り、gateway の拒否行（status と JSON の code）を読む。
function upgrade({ port }, pathname, headers) {
  return new Promise((resolve, reject) => {
    const socket = connect(port, "127.0.0.1");
    let response = "";
    socket.on("data", (chunk) => {
      response += chunk;
    });
    socket.once("close", () => {
      const status = Number(/^HTTP\/1\.1 (\d{3})/.exec(response)?.[1]);
      const code = /\{"code":"([a-z_]+)"\}/.exec(response)?.[1] ?? null;
      resolve({ status, code });
    });
    socket.once("error", reject);
    const lines = Object.entries({
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Version": "13",
      "Sec-WebSocket-Key": "AAAAAAAAAAAAAAAAAAAAAA==",
      ...headers,
    }).map(([k, v]) => `${k}: ${v}\r\n`);
    socket.write(`GET ${pathname} HTTP/1.1\r\n${lines.join("")}\r\n`);
  });
}

test("public origins: only https origins without a path are accepted", () => {
  assert.deepEqual(parsePublicOrigins(""), []);
  assert.deepEqual(parsePublicOrigins(` https://${PUBLIC} , HTTPS://Other.Example:8443/ `), [
    `https://${PUBLIC}`,
    "https://other.example:8443",
  ]);
  assert.deepEqual(parsePublicOrigins(`https://${PUBLIC}:443`), [`https://${PUBLIC}`]);
  for (const bad of [`http://${PUBLIC}`, PUBLIC, `https://${PUBLIC}/app`, "https://a b", "https://x:99999"])
    assert.throws(() => parsePublicOrigins(bad), /CELERIS_WEB_PUBLIC_ORIGINS/);
  // 公開入口には login が要る。
  const bind = { host: "127.0.0.1", port: 7720 };
  assert.throws(() => validateConfig({ bind, publicOrigins: [`https://${PUBLIC}`] }), /PASSWORD_FILE/);
  validateConfig({ bind, passwordFile, publicOrigins: [`https://${PUBLIC}`] });
});

test("policy: https only for loopback peers with a listed Host and no non-https forwarded proto", () => {
  const policy = createOriginPolicy({ publicOrigins: [`https://${PUBLIC}`] });
  const req = (host, remoteAddress = "127.0.0.1", headers = {}) => ({
    socket: { remoteAddress, encrypted: false },
    headers: { host, ...headers },
  });
  assert.equal(policy.selfOrigin(req(PUBLIC, "127.0.0.1", { "x-forwarded-proto": "https" })), `https://${PUBLIC}`);
  assert.equal(policy.selfOrigin(req(PUBLIC, "::ffff:127.0.0.1")), `https://${PUBLIC}`);
  assert.equal(policy.selfOrigin(req(`${PUBLIC}:443`, "::1")), `https://${PUBLIC}`);
  assert.equal(policy.secure(req(PUBLIC)), true);
  // 前段でない接続元・proto が http・一覧に無い Host は従来どおり平文の自 origin。
  assert.equal(policy.selfOrigin(req(PUBLIC, "100.64.0.9")), `http://${PUBLIC}`);
  assert.equal(policy.selfOrigin(req(PUBLIC, "127.0.0.1", { "x-forwarded-proto": "http" })), `http://${PUBLIC}`);
  assert.equal(
    policy.selfOrigin(req("evil.example", "127.0.0.1", { "x-forwarded-proto": "https" })),
    "http://evil.example",
  );
  assert.equal(policy.secure(req("127.0.0.1:7720")), false);
  // 設定が無ければ X-Forwarded-Proto は効かない。
  const none = createOriginPolicy();
  assert.equal(none.selfOrigin(req(PUBLIC, "127.0.0.1", { "x-forwarded-proto": "https" })), `http://${PUBLIC}`);
  assert.equal(none.secure(req(PUBLIC, "127.0.0.1", { "x-forwarded-proto": "https" })), false);
  // TLS の接続は従来どおり https。
  assert.equal(none.selfOrigin({ socket: { encrypted: true }, headers: { host: "x.example" } }), "https://x.example");
  assert.equal(isLoopbackAddress("127.0.0.2"), true);
  assert.equal(isLoopbackAddress("10.0.0.1"), false);
  assert.equal(isLoopbackAddress(undefined), false);
});

test("proxied: login sets a Secure cookie and POST from the public origin passes CSRF", async () => {
  const res = await login(proxied, viaProxy);
  assert.equal(res.status, 200, res.text);
  assert.match(res.cookie, /; Secure/i);
  assert.match(res.cookie, /HttpOnly/i);
  assert.match(res.cookie, /SameSite=Strict/i);
  const cookie = res.cookie.split(";")[0];
  const logout = await request(proxied, "POST", "/logout", {
    headers: { ...viaProxy, cookie, accept: "application/json" },
  });
  assert.equal(logout.status, 200);
  assert.match(logout.headers["set-cookie"][0], /; Secure/i);
});

test("proxied: other origins are rejected as before", async () => {
  const cases = [
    // http の Origin（自 origin は https）。
    { ...viaProxy, origin: `http://${PUBLIC}` },
    // 一覧に無い host（Host は許可済みでも https とはみなさない）。
    { host: "other.example.ts.net", origin: "https://other.example.ts.net", "x-forwarded-proto": "https" },
    // 前段が http と言っている。
    { ...viaProxy, "x-forwarded-proto": "http" },
    // 別 site。
    { ...viaProxy, origin: "https://evil.example" },
  ];
  for (const headers of cases) {
    const res = await request(proxied, "POST", "/login", { headers, body: { password: "pw" } });
    assert.equal(res.status, 403, JSON.stringify(headers));
    assert.equal(res.text, "csrf rejected");
  }
  // 未知の Host は Host 検査で 400。
  const unknown = await request(proxied, "POST", "/login", {
    headers: { host: "evil.example", origin: "https://evil.example", "x-forwarded-proto": "https" },
    body: { password: "pw" },
  });
  assert.equal(unknown.status, 400);
});

test("proxied: browser-live origin checks and device cookie use the public origin", async () => {
  const { cookie: raw } = await login(proxied, viaProxy);
  const cookie = raw.split(";")[0];
  const ok = await request(proxied, "POST", "/browser/owner-session", { headers: { ...viaProxy, cookie } });
  assert.equal(ok.status, 200, ok.text);
  const wrong = await request(proxied, "POST", "/browser/owner-session", {
    headers: { host: PUBLIC, origin: `http://${PUBLIC}`, "x-forwarded-proto": "https", cookie },
  });
  // CSRF の middleware が先に落とす（403）。
  assert.equal(wrong.status, 403);
  // 壊れた端末 cookie は消される。消す cookie にも Secure が付く。
  const resume = await request(proxied, "POST", "/browser/owner-session/resume", {
    headers: { ...viaProxy, cookie: `${cookie}; ${DEVICE_COOKIE_NAME}=broken` },
  });
  assert.equal(resume.status, 403);
  assert.equal(JSON.parse(resume.text).code, "device_rejected");
  assert.match(resume.headers["set-cookie"].join("\n"), new RegExp(`${DEVICE_COOKIE_NAME}=;.*Secure`, "i"));
  // WebSocket の upgrade（live frames）の Origin 検査。
  const ws = { Host: PUBLIC, "X-Forwarded-Proto": "https", Cookie: cookie };
  assert.deepEqual(await upgrade(proxied, "/browser/live/T1/R1/frames", { ...ws, Origin: `http://${PUBLIC}` }), {
    status: 403,
    code: "origin_mismatch",
  });
  const passed = await upgrade(proxied, "/browser/live/T1/R1/frames", { ...ws, Origin: `https://${PUBLIC}` });
  assert.notEqual(passed.code, "origin_mismatch");
});

test("direct http://127.0.0.1 keeps working without Secure", async () => {
  const host = `127.0.0.1:${direct.port}`;
  const res = await login(direct, { host, origin: `http://${host}` });
  assert.equal(res.status, 200);
  assert.doesNotMatch(res.cookie, /Secure/i);
  // 前段の印を付けても、公開 origin が未設定なら https とみなさない。
  const forged = await request(direct, "POST", "/login", {
    headers: { host, origin: `https://${host}`, "x-forwarded-proto": "https" },
    body: { password: "pw" },
  });
  assert.equal(forged.status, 403);
  // 公開 origin を設定した gateway でも、127.0.0.1 直の扱いは変わらない。
  const local = `127.0.0.1:${proxied.port}`;
  const same = await login(proxied, { host: local, origin: `http://${local}` });
  assert.equal(same.status, 200);
  assert.doesNotMatch(same.cookie, /Secure/i);
});

test("manifest and icons are served before login with the HTML CSP allowing the manifest", async () => {
  const host = `127.0.0.1:${proxied.port}`;
  const manifest = await request(proxied, "GET", "/manifest.webmanifest", { headers: { host } });
  assert.equal(manifest.status, 200);
  assert.match(manifest.headers["content-type"], /^application\/manifest\+json/);
  assert.equal(JSON.parse(manifest.text).name, "Celeris");
  const icon = await request(proxied, "GET", "/icons/icon-192.png", { headers: { host } });
  assert.equal(icon.status, 200);
  assert.match(icon.headers["content-type"], /^image\/png/);
  assert.equal((await request(proxied, "GET", "/icons/missing.png", { headers: { host } })).status, 404);
  const html = await request(proxied, "GET", "/", { headers: { host } });
  assert.match(html.headers["content-security-policy"], /manifest-src 'self'/);
});
