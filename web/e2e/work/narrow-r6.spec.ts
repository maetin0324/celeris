/// <reference lib="dom" />
import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";
import { applyStateRoute, stateByKey } from "../support/states";

// fix-r6 narrow: after-r4 の目視で要修正・保留になった狭い幅の 7 点を固定する（rich fixture、screenshots と同じ data）。
// (2) /tasks/T1 360: tab 列の「成果物」が右で切れた → 余白を詰め、収まらない幅は折り返す。
// (3) /graph 360〜412: 横に続く graph の手がかりが無かった → 注記と、見えていない側の端の影。
// (4) /tasks/T1/changes: 長い共通 prefix が毎行反復した → 共通の場所を 1 度だけ出し、各行はファイル名を先に。

async function settle(page: Page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

test("(2) /tasks/T1 360: 5 つの tab が全部 viewport の中に全文で出る", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/tasks/T1`);
    const nav = page.getByRole("navigation", { name: "タスクの表示" });
    const tabs = nav.locator("[data-tab]");
    await expect(tabs).toHaveCount(5);
    const box = await nav.evaluate((el) => ({ scrollWidth: el.scrollWidth, clientWidth: el.clientWidth }));
    expect(box.scrollWidth).toBeLessThanOrEqual(box.clientWidth + 1);
    for (let i = 0; i < 5; i += 1) {
      const tab = tabs.nth(i);
      await expect(tab).toBeInViewport({ ratio: 1 });
      const right = await tab.evaluate((el) => el.getBoundingClientRect().right);
      expect(right).toBeLessThanOrEqual(360);
    }
    await expect(nav.getByRole("link", { name: "成果物" })).toBeInViewport({ ratio: 1 });
  } finally {
    await gateway.close();
  }
});

for (const width of [360, 412]) {
  test(`(3) /graph ${width}: 横に続くことの注記と端の影があり、右端まで動かすと影が左へ移る`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/graph`);
      const canvas = page.getByTestId("graph-canvas");
      await expect(canvas.locator("[data-graph-node='T2']")).toBeAttached();
      await expect(page.getByTestId("graph-scroll-hint")).toBeVisible();
      await expect(page.getByTestId("graph-scroll-hint")).toContainText("横に続きます");
      await expect(canvas).toHaveAttribute("aria-describedby", "graph-scroll-hint");
      await expect(page.getByTestId("graph-edge-end")).toBeVisible();
      await expect(page.getByTestId("graph-edge-start")).toHaveCount(0);
      await canvas.evaluate((el) => {
        el.scrollLeft = el.scrollWidth;
      });
      await expect(page.getByTestId("graph-edge-end")).toHaveCount(0);
      await expect(page.getByTestId("graph-edge-start")).toBeVisible();
    } finally {
      await gateway.close();
    }
  });
}

test("(3) /graph 1440: graph が枠に収まれば注記と影は出ない", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 800 });
    await page.goto(`${gateway.base}/graph`);
    const canvas = page.getByTestId("graph-canvas");
    await expect(canvas.locator("[data-graph-node='T2']")).toBeVisible();
    await settle(page);
    const fits = await canvas.evaluate((el) => el.scrollWidth <= el.clientWidth + 1);
    expect(fits).toBe(true);
    await expect(page.getByTestId("graph-scroll-hint")).toHaveCount(0);
    await expect(page.getByTestId("graph-edge-end")).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});

for (const width of [360, 1440]) {
  test(`(4) /tasks/T1/changes ${width}: 共通の場所は 1 度だけ、各行はファイル名を先に出し全文 path を名前に持つ`, async ({
    page,
  }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/tasks/T1/changes`);
      const files = page.getByTestId("changed-files").locator("[data-file]");
      await expect(files).toHaveCount(15);
      const common = page.getByTestId("changed-files-common");
      await expect(common).toHaveCount(1);
      const prefix = ((await common.locator("span").textContent()) ?? "").trim();
      expect(prefix).toMatch(/\/$/);
      expect(prefix.length).toBeGreaterThan(20);
      for (let i = 0; i < 15; i += 1) {
        const file = files.nth(i);
        const path = (await file.getAttribute("data-file")) ?? "";
        expect(path.startsWith(prefix)).toBe(true);
        const name = file.locator("[data-file-name]");
        await expect(name).toHaveText(path.split("/").at(-1) ?? "");
        await expect(file).toHaveAccessibleName(path);
        await expect(file).toHaveAttribute("title", path);
        // ファイル名は 1 行（共通 prefix の折り返しで縦に伸びない）。
        const metrics = await name.evaluate((el) => ({
          height: el.getBoundingClientRect().height,
          lineHeight: Number.parseFloat(getComputedStyle(el).lineHeight),
        }));
        expect(metrics.height).toBeLessThanOrEqual(metrics.lineHeight + 2);
      }
    } finally {
      await gateway.close();
    }
  });
}

