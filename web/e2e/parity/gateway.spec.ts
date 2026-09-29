import { execFileSync } from "node:child_process";
import http from "node:http";
import { expect, test } from "@playwright/test";
import { createApp } from "../../server/app.js";

let server: http.Server;
let base: string;

test.beforeAll(async () => {
  server = createApp({ log: () => {} }).listen(0, "127.0.0.1");
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
});

function rawGet(route: string, host: string): Promise<http.IncomingMessage> {
  return new Promise((resolve, reject) => {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
    const req = http.get(
      { hostname: "127.0.0.1", port: address.port, path: route, headers: { Host: host } },
      (response) => {
        response.resume();
        resolve(response);
      },
    );
    req.once("error", reject);
  });
}

test("parity-x: 型の再生成差分ゼロ・gui import なし", () => {
  execFileSync(process.execPath, ["scripts/gen-types.mjs", "--check"]);
  execFileSync(process.execPath, ["scripts/check-boundaries.mjs"]);
});

test("parity: /healthz 未認証 200 と版", async () => {
  const response = await fetch(`${base}/healthz`);
  expect(response.status).toBe(200);
  expect(await response.json()).toMatchObject({ ok: true, name: "celeris-web", version: "0.1.0" });
  const html = await fetch(`${base}/login`);
  expect(html.status).toBe(200);
  expect(html.headers.get("cache-control")).toBe("no-store");
});

test("parity-x: CSRF 別 origin の変更は 403", async () => {
  for (const headers of [
    { Origin: "http://evil.example" },
    { Origin: "http://127.0.0.1:9999" },
    { "Sec-Fetch-Site": "same-site" },
  ])
    expect((await fetch(`${base}/api/unknown`, { method: "POST", headers })).status).toBe(403);
  expect((await fetch(`${base}/api/unknown`, { method: "POST" })).status).toBe(404);
});

test("parity-x: Host 全経路で 400", async () => {
  for (const route of ["/", "/healthz", "/assets/no.js", "/api/no", "/files/no", "/events", "/missing"])
    expect((await rawGet(route, "evil.example")).statusCode).toBe(400);
});

test("parity-x: 全応答の security header", async () => {
  for (const route of ["/", "/healthz", "/api/no", "/files/no", "/events", "/assets/no.js"] as const) {
    const response = await fetch(`${base}${route}`);
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    expect(response.headers.get("referrer-policy")).toBe("no-referrer");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    expect(response.headers.get("content-security-policy")).toContain("frame-ancestors 'none'");
  }
});
