import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { createApp } from "./app.js";

test("site policy and credential grant mutations require owner session and CSRF; generic relay cannot bypass", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "browser-settings-"));
  const requests = [];
  let conflict = false;
  const daemon = http.createServer((req, res) => {
    let body = "";
    req.on("data", (data) => {
      body += data;
    });
    req.on("end", () => {
      requests.push({ method: req.method, path: req.url, body: body ? JSON.parse(body) : null });
      res.setHeader("content-type", "application/json");
      if (conflict) {
        res.statusCode = 409;
        return res.end(JSON.stringify({ code: "site_policy_in_use" }));
      }
      if (req.method === "DELETE") {
        res.statusCode = 204;
        return res.end();
      }
      res.end(JSON.stringify({ id: "browser-execution", created: true, policy: body ? JSON.parse(body) : null }));
    });
  });
  let gateway;
  try {
    writeFileSync(path.join(dir, "index.html"), "<title>test</title>");
    writeFileSync(path.join(dir, "pw"), "pw", { mode: 0o600 });
    daemon.listen(0, "127.0.0.1");
    await new Promise((resolve) => daemon.once("listening", resolve));
    const app = createApp({
      distDir: dir,
      passwordFile: path.join(dir, "pw"),
      daemonUrl: `http://127.0.0.1:${daemon.address().port}`,
      log: () => {},
    });
    gateway = app.listen(0, "127.0.0.1");
    await new Promise((resolve) => gateway.once("listening", resolve));
    const base = `http://127.0.0.1:${gateway.address().port}`;
    const login = await fetch(`${base}/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: '{"password":"pw"}',
    });
    const cookie = login.headers.getSetCookie()[0].split(";")[0];
    const call = (route, method, body) =>
      fetch(base + route, {
        method,
        headers: { cookie, origin: base, "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    assert.equal((await call("/browser/settings", "GET")).status, 200);
    assert.equal((await call("/browser/settings", "HEAD")).status, 200);
    const body = {
      exact_origin: "https://courses.example.com",
      login_url: "https://courses.example.com/login",
      password_selector: "#password",
      submit_selector: null,
    };
    assert.equal((await call("/browser/site-policies/manaba", "PUT", body)).status, 403);
    assert.equal((await call("/browser/settings", "PATCH", { credential_use: true })).status, 403);
    assert.equal(requests.length, 0);
    const challenge = await (await call("/browser/owner-session", "POST")).json();
    assert.equal(app.locals.browserLive.approve(challenge.challenge), true);
    const { csrfToken: csrf } = await (await call("/browser/owner-session", "GET")).json();
    assert.equal((await call("/browser/site-policies/manaba", "PUT", { ...body, csrf: "wrong" })).status, 403);
    assert.equal((await call("/browser/settings", "PATCH", { credential_use: true, csrf: "wrong" })).status, 403);
    assert.equal(requests.length, 0);
    assert.equal((await call("/browser/site-policies/manaba", "PUT", { ...body, csrf })).status, 200);
    assert.deepEqual(requests[0], { method: "PUT", path: "/api/v1/browser/site-policies/manaba", body });
    assert.equal(
      (await call("/browser/settings", "PATCH", { credential_use: true, credential_policy_ids: ["manaba"], csrf }))
        .status,
      200,
    );
    assert.deepEqual(requests[1].body, { credential_use: true, credential_policy_ids: ["manaba"] });
    for (const [route, method] of [
      ["/api/browser/site-policies/manaba", "PUT"],
      ["/api/browser/site-policies/manaba", "DELETE"],
      ["/api/org/browser-execution/browser-settings", "PATCH"],
      ["/api/browser/site-policies/%6Danaba", "PUT"],
      ["/api/browser/%73ite-policies/manaba", "DELETE"],
      ["/api/org/browser-execution/%62rowser-settings", "PATCH"],
      ["/api/org/%62rowser-execution/browser-settings", "PATCH"],
    ])
      assert.equal((await call(route, method, { ...body, csrf })).status, 403);
    assert.equal(requests.length, 2);
    conflict = true;
    const rejected = await call("/browser/site-policies/manaba", "DELETE", { csrf });
    assert.equal(rejected.status, 409);
    assert.equal((await rejected.json()).code, "site_policy_in_use");
    conflict = false;
    assert.equal((await call("/browser/site-policies/manaba", "DELETE", { csrf })).status, 200);
    assert.equal(requests.at(-1).body, null);
    const response = await call("/browser/settings", "PATCH", {
      credential_use: false,
      credential_policy_ids: [],
      csrf,
    });
    assert.equal(response.status, 200);
    assert.deepEqual(requests.at(-1).body, { credential_use: false, credential_policy_ids: [] });
  } finally {
    gateway?.closeAllConnections();
    if (gateway) await new Promise((resolve) => gateway.close(resolve));
    daemon.closeAllConnections();
    await new Promise((resolve) => daemon.close(resolve));
    rmSync(dir, { recursive: true, force: true });
  }
});
