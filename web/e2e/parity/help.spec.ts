import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

const dir = makeTmpDir("celeris-help-");
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
      (request) =>
        ![
          "/api/v1/health",
          "/api/v1/daemon",
          "/api/v1/inbox/items",
          "/api/v1/notifications/unread-count",
          "/api/v1/stream",
        ].includes(request.path),
    ),
  ).toEqual([]);
});

test("parity-x: /help 見出しは h1→h2→h3 の順で、本文は読みやすい幅", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${gateway.base}/help`);
  await expect(page.getByRole("heading", { level: 1, name: "ヘルプ" })).toBeVisible();
  const levels = await page
    .locator('[data-screen="/help"]')
    .locator("h1, h2, h3, h4")
    .evaluateAll((nodes) => nodes.map((node) => Number(node.tagName.slice(1))));
  expect(levels[0]).toBe(1);
  expect(levels).toContain(3);
  // 見出しの段を飛ばさない（h2 の無い h3 を作らない）。
  for (let i = 1; i < levels.length; i++) expect(levels[i] - levels[i - 1]).toBeLessThanOrEqual(1);
  const width = await page.locator("section#flow").evaluate((node) => node.getBoundingClientRect().width);
  expect(width).toBeLessThanOrEqual(720);
});
