import { execFileSync } from "node:child_process";
import { readdirSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { makeTmpDir } from "../support/tmp-dir";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

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

function rawGet(route: string, host: string, target: http.Server = server): Promise<http.IncomingMessage> {
  return new Promise((resolve, reject) => {
    const address = target.address();
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
  const cases: Record<string, string>[] = [
    { Origin: "http://evil.example" },
    { Origin: "http://127.0.0.1:9999" },
    { "Sec-Fetch-Site": "same-site" },
  ];
  for (const headers of cases)
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

test.describe("P5-02 全経路 security gate", () => {
  const password = "security-gate-password";
  const dir = makeTmpDir("celeris-web-security-gate-");
  const passwordFile = path.join(dir, "password");
  const tokenFile = path.join(dir, "token");
  writeFileSync(passwordFile, `${password}\n`);
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  let gateServer: http.Server;
  let gateBase: string;
  let asset: string;

  test.beforeAll(async () => {
    const daemonUrl = await daemon.start();
    const assets = readdirSync(path.join(import.meta.dirname, "..", "..", "dist", "assets"));
    asset = `/assets/${assets.find((name) => /-[a-zA-Z0-9_-]{8,}\./.test(name)) ?? assets[0]}`;
    gateServer = createApp({
      passwordFile,
      daemonUrl,
      daemonTokenFile: tokenFile,
      relayTimeoutMs: 1_000,
      log: () => {},
    }).listen(0, "127.0.0.1");
    await new Promise<void>((resolve, reject) => {
      gateServer.once("listening", resolve);
      gateServer.once("error", reject);
    });
    const address = gateServer.address();
    if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
    gateBase = `http://127.0.0.1:${address.port}`;
  });

  test.afterAll(async () => {
    if (gateServer) {
      gateServer.closeAllConnections();
      await new Promise<void>((resolve) => gateServer.close(() => resolve()));
    }
    await daemon.close().catch(() => {});
    rmSync(dir, { recursive: true, force: true });
  });

  function routes() {
    return [
      asset,
      "/",
      "/login",
      "/no-such-page",
      "/api/health",
      "/files/tasks/T1/artifacts/0",
      "/events",
      "/console/stream",
      "/api/console/stream",
    ];
  }

  test("parity-x: security 全経路 Host・CSRF・session・header・token", async () => {
    const cookieResponse = await fetch(`${gateBase}/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ password }),
    });
    expect(cookieResponse.status).toBe(200);
    const cookie = (cookieResponse.headers.get("set-cookie") ?? "").split(";")[0];
    expect(cookie).toMatch(/^__celeris_web_session=/);

    for (const route of routes()) {
      expect((await rawGet(route, "evil.example", gateServer)).statusCode, `${route} Host`).toBe(400);
      const csrf = await fetch(`${gateBase}${route}`, {
        method: "POST",
        headers: { Origin: "http://evil.example", Cookie: cookie },
        redirect: "manual",
      });
      expect(csrf.status, `${route} CSRF`).toBe(403);
      expect(await csrf.text(), `${route} CSRF body`).not.toContain(FIXTURE_TOKEN);
      expect(
        (
          await fetch(`${gateBase}${route}`, {
            method: "POST",
            headers: { "Sec-Fetch-Site": "same-site", Cookie: cookie },
          })
        ).status,
        `${route} Fetch Metadata`,
      ).toBe(403);

      const protectedRoute =
        route.startsWith("/api/") || route.startsWith("/files/") || route === "/events" || route === "/console/stream";
      const anonymous = await fetch(`${gateBase}${route}`, { redirect: "manual" });
      expect(anonymous.status, `${route} session`).toBe(protectedRoute ? 401 : route === "/no-such-page" ? 404 : 200);
      expect(anonymous.headers.get("x-content-type-options"), route).toBe("nosniff");
      expect(anonymous.headers.get("referrer-policy"), route).toBe("no-referrer");
      expect(anonymous.headers.get("x-frame-options"), route).toBe("DENY");
      expect(anonymous.headers.get("content-security-policy"), route).toContain("frame-ancestors 'none'");
      expect(anonymous.headers.get("cache-control"), route).not.toBeNull();
      expect(await anonymous.text(), `${route} anonymous body`).not.toContain(FIXTURE_TOKEN);
      if (protectedRoute)
        expect(
          (await fetch(`${gateBase}${route}`, { headers: { Cookie: `${cookie}tampered` } })).status,
          `${route} bad cookie`,
        ).toBe(401);
    }

    // 認証後の中継応答も token を browser へ渡さない。stream は本文を読まず切断する。
    for (const route of ["/api/health", "/files/tasks/T1/artifacts/0", "/api/console/stream"]) {
      const response = await fetch(`${gateBase}${route}`, { headers: { Cookie: cookie } });
      expect(response.headers.get("x-content-type-options"), route).toBe("nosniff");
      expect(response.headers.get("referrer-policy"), route).toBe("no-referrer");
      expect(response.headers.get("x-frame-options"), route).toBe("DENY");
      expect(JSON.stringify([...response.headers]), route).not.toContain(FIXTURE_TOKEN);
      expect(await response.text(), route).not.toContain(FIXTURE_TOKEN);
    }
    for (const route of ["/events", "/console/stream"]) {
      const controller = new AbortController();
      const response = await fetch(`${gateBase}${route}`, { headers: { Cookie: cookie }, signal: controller.signal });
      expect(response.status, route).toBe(200);
      expect(response.headers.get("x-content-type-options"), route).toBe("nosniff");
      expect(response.headers.get("referrer-policy"), route).toBe("no-referrer");
      expect(response.headers.get("x-frame-options"), route).toBe("DENY");
      expect(JSON.stringify([...response.headers]), route).not.toContain(FIXTURE_TOKEN);
      const firstChunk = await response.body?.getReader().read();
      expect(new TextDecoder().decode(firstChunk?.value), route).not.toContain(FIXTURE_TOKEN);
      controller.abort();
    }

    await daemon.close();
    for (const route of ["/login", "/"]) {
      const response = await fetch(`${gateBase}${route}`);
      expect(response.status, `${route} daemon stopped`).toBe(200);
      expect(response.headers.get("content-type"), route).toContain("text/html");
    }
  });
});
