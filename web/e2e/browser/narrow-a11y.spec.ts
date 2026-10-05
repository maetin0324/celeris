import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startBrowserGateway } from "../support/browser-gateway";

test.use({ viewport: { width: 360, height: 800 }, bypassCSP: true });

async function top(page: Page, selector: string) {
  const box = await page.locator(selector).first().boundingBox();
  expect(box, selector).not.toBeNull();
  if (!box) throw new Error(`missing element: ${selector}`);
  return box.y;
}

async function checkTargets(page: Page, selector: string) {
  for (const target of await page.locator(`${selector} a, ${selector} button`).all()) {
    if (!(await target.isVisible())) continue;
    const box = await target.boundingBox();
    expect(box?.width ?? 0, (await target.textContent()) ?? "").toBeGreaterThanOrEqual(44);
    expect(box?.height ?? 0, (await target.textContent()) ?? "").toBeGreaterThanOrEqual(44);
  }
}

async function capture(page: Page, name: string) {
  const directory = process.env.BROWSER_SHOT_DIR;
  if (!directory) return;
  mkdirSync(directory, { recursive: true });
  await page.screenshot({ path: path.join(directory, name), fullPage: true });
}

async function captureOtherWidths(page: Page, stem: string) {
  for (const width of [390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth), `${stem} @${width}`).toBeLessThanOrEqual(
      width,
    );
    await capture(page, `${stem}-${width}.png`);
  }
}

test("360px browser list stacks run state before links, with accessible targets", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser`);
    const row = page.locator('[data-testid="browser-run-row"][data-run-id="R1"]');
    await expect(row.getByTestId("browser-run-lease")).toHaveText("監視のみ");
    await expect(page.getByTestId("browser-waits")).toBeVisible();
    expect(await top(page, "[data-testid=browser-runs]")).toBeLessThan(await top(page, "[data-testid=browser-waits]"));
    expect(await top(page, "[data-testid=browser-run-state]")).toBeLessThan(
      await top(page, "[data-testid=browser-run-open]"),
    );
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
    await checkTargets(page, "[data-testid=browser-runs]");
    await checkTargets(page, "[data-testid=browser-waits]");
    expect(await seriousViolations(page)).toEqual([]);
    await capture(page, "browser-list-360.png");
    await captureOtherWidths(page, "browser-list");
  } finally {
    await gateway.close();
  }
});

test("360px run puts the sticky control bar before Live View, waits and events", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(page.getByTestId("browser-control-phase")).toHaveText("監視のみ — エージェントが操作中");
    await expect(page.getByTestId("browser-live-iframe")).toBeVisible();
    const selectors = [
      "[data-testid=browser-control-phase]",
      "[data-testid=browser-control-bar] button",
      "[data-testid=browser-live-view]",
      "[data-testid=browser-run-waits]",
      "[data-testid=browser-live-events]",
    ];
    const positions = await Promise.all(selectors.map((selector) => top(page, selector)));
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
    expect(await page.getByTestId("browser-control-bar").evaluate((el) => getComputedStyle(el).position)).toBe(
      "sticky",
    );
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
    await checkTargets(page, "[data-testid=browser-control-bar]");
    await checkTargets(page, "[data-testid=browser-live-view]");
    await checkTargets(page, "[data-testid=browser-run-waits]");
    expect(await seriousViolations(page)).toEqual([]);
    await capture(page, "browser-run-360.png");
    await captureOtherWidths(page, "browser-run");
  } finally {
    await gateway.close();
  }
});
