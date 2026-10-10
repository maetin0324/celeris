import assert from "node:assert/strict";
import { generateKeyPairSync, verify } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { createApp } from "./app.js";

test("saved credentials require owner and CSRF and bind signed deletion to the selected ID", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "saved-credentials-"));
  const keys = generateKeyPairSync("ed25519");
  const requests = [];
  const daemon = http.createServer((req, res) => {
    const payload = req.headers["x-celeris-assertion-payload"];
    assert.equal(
      verify(
        null,
        Buffer.from(payload),
        keys.publicKey,
        Buffer.from(req.headers["x-celeris-assertion-signature"], "hex"),
      ),
      true,
    );
    requests.push({ path: req.url, claims: JSON.parse(payload) });
    res.setHeader("content-type", "application/json");
    if (req.method === "DELETE") {
      res.statusCode = 204;
      return res.end();
    }
    res.end(
      JSON.stringify({ items: [{ credential_id: "cred-saved", policy_id: "manaba", created_at: 1, expires_at: 2 }] }),
    );
  });
  let gateway;
  try {
    writeFileSync(path.join(dir, "index.html"), "test");
    writeFileSync(path.join(dir, "pw"), "pw", { mode: 0o600 });
    writeFileSync(path.join(dir, "key"), keys.privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
    daemon.listen(0, "127.0.0.1");
    await new Promise((resolve) => daemon.once("listening", resolve));
    const app = createApp({
      distDir: dir,
      passwordFile: path.join(dir, "pw"),
      attestationKeyFile: path.join(dir, "key"),
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
    assert.equal((await call("/browser/credentials", "GET")).status, 403);
    assert.equal((await call("/browser/credentials/cred-saved", "DELETE", {})).status, 403);
    assert.equal(requests.length, 0);
    const challenge = await (await call("/browser/owner-session", "POST")).json();
    assert.equal(app.locals.browserLive.approve(challenge.challenge), true);
    const { csrfToken: csrf } = await (await call("/browser/owner-session", "GET")).json();
    const list = await call("/browser/credentials", "GET");
    assert.equal(list.status, 200);
    assert.equal(list.headers.get("cache-control"), "no-store");
    assert.equal((await list.json()).items[0].policy_id, "manaba");
    assert.equal(requests[0].claims.purpose, "credential_list");
    assert.equal(requests[0].claims.actor_id, "owner");
    assert.equal(requests[0].claims.owner_session, true);
    assert.equal((await call("/browser/credentials/cred-saved", "DELETE", { csrf: "wrong" })).status, 403);
    assert.equal((await call("/api/browser/credentials", "GET")).status, 403);
    assert.equal((await call("/api/browser/credentials/cred-saved", "DELETE", {})).status, 403);
    assert.equal((await call("/browser/credentials/cred-saved", "DELETE", { csrf })).status, 200);
    assert.equal(requests.length, 2);
    assert.equal(requests[1].claims.purpose, "credential_delete");
    assert.equal(requests[1].claims.credential_id, "cred-saved");
  } finally {
    if (gateway) await new Promise((resolve) => gateway.close(resolve));
    await new Promise((resolve) => daemon.close(resolve));
    rmSync(dir, { recursive: true, force: true });
  }
});
