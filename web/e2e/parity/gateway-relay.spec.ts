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
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  files: {
    "/api/v1/tasks/T1/runs/R1/stdout.jsonl": { body: '{"n":1}\n{"n":2}\n', type: "application/x-ndjson" },
    "/api/v1/tasks/T1/artifacts/0": { body: "# report\n", type: "text/markdown" },
    "/api/v1/tasks/T1/artifacts/1": {
      body: "<script>document.title='ran'</script><p>html</p>",
      type: "text/html",
      disposition: 'inline; filename="page.html"',
    },
  },
});

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

test("parity: files runs 中継・Range・offset・不正 name", async () => {
  const full = await fetch(`${base}/files/tasks/T1/runs/R1/stdout.jsonl?offset=0&length=100`);
  expect(full.status).toBe(200);
  expect(await full.text()).toBe('{"n":1}\n{"n":2}\n');
  expect(full.headers.get("x-content-type-options")).toBe("nosniff");
  expect(daemon.requests.at(-1)?.authorization).toBe(`Bearer ${FIXTURE_TOKEN}`);

  const partial = await fetch(`${base}/files/tasks/T1/runs/R1/stdout.jsonl`, { headers: { Range: "bytes=8-" } });
  expect(partial.status).toBe(206);
  expect(partial.headers.get("content-range")).toBe("bytes 8-15/16");
  expect(await partial.text()).toBe('{"n":2}\n');
  const outside = await fetch(`${base}/files/tasks/T1/runs/R1/stdout.jsonl`, { headers: { Range: "bytes=99-" } });
  expect(outside.status).toBe(416);
  expect(outside.headers.get("content-range")).toBe("bytes */16");

  for (const bad of ["%2e%2e", "a%2Fb", "a%5Cb", "a%00b"]) {
    const res = await fetch(`${base}/files/tasks/T1/runs/R1/${bad}`);
    expect(res.status, bad).toBe(400);
  }
});

test("parity: files artifacts 中継・download・不正 idx", async () => {
  const md = await fetch(`${base}/files/tasks/T1/artifacts/0?download=1`);
  expect(md.status).toBe(200);
  expect(await md.text()).toBe("# report\n");
  for (const bad of ["x", "-1", "1.5", "%2e%2e"]) {
    const res = await fetch(`${base}/files/tasks/T1/artifacts/${bad}`);
    expect(res.status, bad).toBe(400);
  }
});

test("parity-x: file 不正 path・header 許可リスト・HTML を実行しない", async ({ page }) => {
  const res = await fetch(`${base}/files/tasks/T1/artifacts/1`);
  expect(res.headers.get("content-disposition")).toBe('attachment; filename="page.html"');
  expect(res.headers.get("x-content-type-options")).toBe("nosniff");
  expect(res.headers.get("content-security-policy")).toMatch(/^sandbox/);

  // ブラウザで開いても download になり、同一オリジンの document として script が走らない。
  await page.goto(`${base}/`);
  const downloaded = page.waitForEvent("download");
  await page.evaluate(`(() => {
    const a = document.createElement("a");
    a.href = "/files/tasks/T1/artifacts/1";
    document.body.append(a);
    a.click();
  })()`);
  expect((await downloaded).suggestedFilename()).toBe("page.html");
  expect(await page.title()).not.toBe("ran");
});
