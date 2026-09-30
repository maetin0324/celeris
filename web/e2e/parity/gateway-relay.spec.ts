import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { checkSecrets, FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";

// gateway の中継（P1-07〜P1-09）。偽 daemon と gateway は loopback の空き port。dist/ は webServer の build が作る。
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-relay-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });

let server: http.Server;
let base: string;

test.beforeAll(async () => {
  const daemonUrl = await daemon.start();
  server = createApp({ daemonUrl, daemonTokenFile: tokenFile, log: () => {} }).listen(0, "127.0.0.1");
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
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

test("parity-x: token が HTML・bundle・エラーに出ない", async ({ page }) => {
  expect(await checkSecrets()).toEqual([]);

  const response = await fetch(`${base}/api/health`, {
    headers: { Authorization: "Bearer browser-supplied", Cookie: "x=y" },
  });
  expect(response.status).toBe(200);
  const request = daemon.requests.at(-1);
  expect(request?.path).toBe("/api/v1/health");
  expect(request?.authorization).toBe(`Bearer ${FIXTURE_TOKEN}`);
  expect(request?.cookie).toBeNull();

  const bodies: string[] = [];
  page.on("response", async (res) => {
    bodies.push(await res.text().catch(() => ""));
  });
  await page.goto(`${base}/`);
  const inPage = await page.evaluate(async () => {
    const api = await fetch("/api/inbox");
    return { status: api.status, text: await api.text() };
  });
  const html = await page.content();
  expect(inPage.status).toBe(200);
  for (const text of [...bodies, inPage.text, html]) expect(text).not.toContain(FIXTURE_TOKEN);
});
