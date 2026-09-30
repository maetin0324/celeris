import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { createApp } from "./app.js";
import { createAuth, SESSION_COOKIE_NAME, SESSION_MAX_AGE_SECONDS, safeNextPath } from "./auth.js";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-auth-"));
const passwordFile = path.join(dir, "password");
const secretFile = path.join(dir, "secret");
writeFileSync(passwordFile, "correct horse\n");
writeFileSync(secretFile, "web-secret\n");
writeFileSync(path.join(dir, "index.html"), "<!doctype html><title>celeris</title>");
let server;
let base;

before(async () => {
  const app = createApp({ passwordFile, secretFile, distDir: dir, failedLoginDelayMs: 1000, log: () => {} });
  server = app.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  base = `http://127.0.0.1:${server.address().port}`;
});
after(async () => {
  await new Promise((resolve) => server.close(resolve));
  rmSync(dir, { recursive: true, force: true });
});

function login(password, next, headers = {}) {
  return fetch(`${base}/login`, {
    method: "POST",
    redirect: "manual",
    headers: { "Content-Type": "application/json", Accept: "application/json", ...headers },
    body: JSON.stringify({ password, next }),
  });
}

function cookieOf(response) {
  const header = response.headers.get("set-cookie") ?? "";
  return header.split(";")[0];
}

test("unauthenticated /api, /files, /events are 401; /login /logout /healthz and HTML pass", async () => {
  for (const route of ["/api/tasks", "/api", "/files/a/b.txt", "/events", "/events/x"])
    assert.equal((await fetch(`${base}${route}`)).status, 401, route);
  assert.equal((await fetch(`${base}/healthz`)).status, 200);
  assert.equal((await fetch(`${base}/login`)).status, 200);
  assert.equal((await fetch(`${base}/logout`, { method: "POST", redirect: "manual" })).status, 303);
  const session = await fetch(`${base}/api/session`);
  assert.equal(session.status, 200);
  assert.deepEqual(await session.json(), { authenticated: false, authRequired: true });
});

test("login success sets a signed HttpOnly SameSite=Strict 24 h cookie and returns the safe next", async () => {
  const response = await login("correct horse", "/tasks/01ABC?tab=runs");
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { ok: true, next: "/tasks/01ABC?tab=runs" });
  const header = response.headers.get("set-cookie") ?? "";
  assert.match(header, new RegExp(`^${SESSION_COOKIE_NAME}=`));
  assert.match(header, /HttpOnly/);
  assert.match(header, /SameSite=Strict/);
  assert.match(header, /Max-Age=86400/);
  assert.doesNotMatch(header, /Secure/);
  const cookie = cookieOf(response);
  const session = await fetch(`${base}/api/session`, { headers: { Cookie: cookie } });
  assert.deepEqual(await session.json(), { authenticated: true, authRequired: true });
  // 認証済みなら中継前の /api/* は gateway の 404（P1-07 まで）。
  assert.equal((await fetch(`${base}/api/tasks`, { headers: { Cookie: cookie } })).status, 404);
});

test("form login redirects with 303 to next", async () => {
  const response = await fetch(`${base}/login`, {
    method: "POST",
    redirect: "manual",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: "password=correct+horse&next=%2Ftasks",
  });
  assert.equal(response.status, 303);
  assert.equal(response.headers.get("location"), "/tasks");
});

test("login failure waits 1 s and issues no cookie", async () => {
  const started = Date.now();
  const response = await login("wrong", "/");
  assert.equal(response.status, 401);
  assert.ok(Date.now() - started >= 950, "failed login must wait about 1 s");
  assert.equal(response.headers.get("set-cookie"), null);
});

test("next accepts only same-origin absolute paths", async () => {
  for (const bad of [
    "//evil.example/x",
    "/\\evil.example",
    "https://evil.example/",
    "javascript:alert(1)",
    "tasks",
    "",
    null,
    "/a\nb",
  ])
    assert.equal(safeNextPath(bad), "/", String(bad));
  assert.equal(safeNextPath("/tasks?x=1#h"), "/tasks?x=1#h");
  assert.equal(safeNextPath("/a/../b"), "/b");
  const response = await login("correct horse", "//evil.example/");
  assert.equal((await response.json()).next, "/");
});

test("login and logout are CSRF-checked", async () => {
  assert.equal((await login("correct horse", "/", { Origin: "http://evil.example" })).status, 403);
  assert.equal((await login("correct horse", "/", { "Sec-Fetch-Site": "same-site" })).status, 403);
  const logout = await fetch(`${base}/logout`, { method: "POST", headers: { Origin: "http://evil.example" } });
  assert.equal(logout.status, 403);
});

test("logout clears the cookie", async () => {
  const response = await fetch(`${base}/logout`, { method: "POST", redirect: "manual" });
  assert.equal(response.status, 303);
  assert.equal(response.headers.get("location"), "/login");
  const header = response.headers.get("set-cookie") ?? "";
  assert.match(header, new RegExp(`^${SESSION_COOKIE_NAME}=;`));
  assert.match(header, /Expires=Thu, 01 Jan 1970/);
});

test("expired, tampered and gui/ cookies are unauthenticated", async () => {
  let clock = Date.now();
  const auth = createAuth({ passwordFile, secretFile, now: () => clock });
  const token = auth.issue();
  assert.ok(auth.isValid(token));
  clock += SESSION_MAX_AGE_SECONDS * 1000;
  assert.equal(auth.isValid(token), false, "expired");
  clock = Date.now();
  const [payload, mac] = token.split(".");
  const forged = Buffer.from(JSON.stringify({ iat: Math.floor(clock / 1000) + 10, id: "x" })).toString("base64url");
  assert.equal(auth.isValid(`${forged}.${mac}`), false, "tampered payload");
  assert.equal(auth.isValid(`${payload}.${mac.slice(0, -1)}A`), false, "tampered mac");
  const future = Buffer.from(JSON.stringify({ iat: Math.floor(clock / 1000) + 3600, id: "x" })).toString("base64url");
  const futureMac = createHmac("sha256", "web-secret").update(future).digest("base64url");
  assert.equal(auth.isValid(`${future}.${futureMac}`), false, "issued in the future");

  const expiredPayload = Buffer.from(
    JSON.stringify({ iat: Math.floor(Date.now() / 1000) - SESSION_MAX_AGE_SECONDS - 1, id: "x" }),
  ).toString("base64url");
  const expiredMac = createHmac("sha256", "web-secret").update(expiredPayload).digest("base64url");
  for (const cookie of [
    `${SESSION_COOKIE_NAME}=${expiredPayload}.${expiredMac}`,
    `${SESSION_COOKIE_NAME}=${payload}.${mac}x`,
    // gui/ の cookie 名と形式（react-router の署名 cookie）は受け入れない。
    `__celeris_gui_session=${encodeURIComponent(Buffer.from('{"iat":1,"id":"x"}').toString("base64"))}.abc`,
    `__celeris_gui_session=${payload}.${mac}`,
  ]) {
    const response = await fetch(`${base}/api/session`, { headers: { Cookie: cookie } });
    assert.equal((await response.json()).authenticated, false, cookie);
    assert.equal((await fetch(`${base}/api/tasks`, { headers: { Cookie: cookie } })).status, 401, cookie);
  }
});

test("a different signing key does not validate", () => {
  const other = mkdtempSync(path.join(tmpdir(), "celeris-web-auth-other-"));
  try {
    writeFileSync(path.join(other, "secret"), "other-secret");
    const a = createAuth({ passwordFile, secretFile });
    const b = createAuth({ passwordFile, secretFile: path.join(other, "secret") });
    assert.equal(b.isValid(a.issue()), false);
  } finally {
    rmSync(other, { recursive: true, force: true });
  }
});

test("without a password file auth is off and the session is open", async () => {
  const auth = createAuth({ passwordFile: undefined, secretFile });
  assert.equal(auth.enabled, false);
  assert.equal(auth.verifyPassword("anything"), false);
});
