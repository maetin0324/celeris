import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-help-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
let gateway: Awaited<ReturnType<typeof startGateway>>;
test.beforeAll(async () => {
  gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
});
test.afterAll(async () => {
  await gateway.close();
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

const ids = ["flow", "screens", "acceptance", "status", "failure", "mcp"];
test("parity: /help 6 節とアンカー", async ({ page }) => {
  const calls: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/help")) calls.push(request.url());
  });
  await page.goto(`${gateway.base}/help#mcp`);
  await expect(page.getByRole("heading", { level: 1, name: "ヘルプ" })).toBeVisible();
  for (const id of ids) {
    await expect(page.locator(`section#${id} h2`)).toBeVisible();
    await expect(page.locator(`nav[aria-label="ヘルプの目次"] a[href="#${id}"]`)).toHaveCount(1);
  }
  await expect(page).toHaveURL(/#mcp$/);
  await page.locator('a[href="#flow"]').click();
  await expect(page).toHaveURL(/#flow$/);
  expect(calls).toEqual([]);
  // Shell の badge/接続状態以外に、help 固有の daemon API を呼ばない。
  expect(
    daemon.requests.filter(
      (request) => !["/api/v1/health", "/api/v1/daemon", "/api/v1/inbox", "/api/v1/stream"].includes(request.path),
    ),
  ).toEqual([]);
});
