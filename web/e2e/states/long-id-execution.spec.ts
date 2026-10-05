import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";
import { LONG_TASK_ID, stateByKey } from "../support/states";

// long-id 状態の task 詳細を 1440 で開き、実行節が取得失敗帯なしで execution と routing を描き、
// 長い ID で文書が横に広がらないことを確かめる（fix-r7.md の長い ID の節）。
test("long-id task detail at 1440 shows the execution section without fetch errors or horizontal scroll", async ({
  page,
}) => {
  const gateway = await startFixtureGateway(stateByKey("long-id").daemon);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/tasks/${LONG_TASK_ID}`);
    await expect(page.locator("h1")).toContainText(LONG_TASK_ID, { timeout: 20_000 });
    const panel = page.getByTestId("execution-panel");
    await expect(panel.getByTestId("execution-view").locator("[data-phase]")).toHaveAttribute(
      "data-phase",
      "verifying",
    );
    await expect(panel.getByTestId("execution-view")).toContainText("v1（2 件）");
    await expect(panel.getByTestId("routing-panel")).toContainText("担当 ui-ux");
    await expect(page.locator("[data-fetch-state='loading']")).toHaveCount(0);
    await expect(panel.getByRole("alert")).toHaveCount(0);
    await expect(panel.getByRole("button", { name: "再試行" })).toHaveCount(0);
    await expect(page.locator("[data-fetch-state='error']")).toHaveCount(0);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow, "横 scroll が出ている").toBeLessThanOrEqual(0);
  } finally {
    await gateway.close();
  }
});
