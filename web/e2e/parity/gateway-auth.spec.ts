import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { createApp } from "../../server/app.js";

// gateway の認証（P1-05 / P1-06）。gateway は空き port の loopback で、パスワードのファイルを与えて認証を有効にする。
const PASSWORD = "e2e-password";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-auth-"));
const passwordFile = path.join(dir, "password");
writeFileSync(passwordFile, `${PASSWORD}\n`);

let server: http.Server;
let base: string;

test.beforeAll(async () => {
  server = createApp({ passwordFile, log: () => {} }).listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
  base = `http://127.0.0.1:${address.port}`;
});

test.afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
  rmSync(dir, { recursive: true, force: true });
});

async function sessionCookie(): Promise<string> {
  const response = await fetch(`${base}/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify({ password: PASSWORD, next: "/" }),
  });
  expect(response.status).toBe(200);
  return (response.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
}

test("parity: /logout cookie 消去と CSRF", async () => {
  const cookie = await sessionCookie();
  const before = (await (await fetch(`${base}/api/session`, { headers: { Cookie: cookie } })).json()) as {
    authenticated: boolean;
  };
  expect(before.authenticated).toBe(true);
  const foreign = await fetch(`${base}/logout`, {
    method: "POST",
    redirect: "manual",
    headers: { Cookie: cookie, Origin: "http://evil.example" },
  });
  expect(foreign.status).toBe(403);
  const logout = await fetch(`${base}/logout`, { method: "POST", redirect: "manual", headers: { Cookie: cookie } });
  expect(logout.status).toBe(303);
  expect(logout.headers.get("location")).toBe("/login");
  expect(logout.headers.get("set-cookie")).toMatch(/^__celeris_web_session=;.*Expires=Thu, 01 Jan 1970/);
});
