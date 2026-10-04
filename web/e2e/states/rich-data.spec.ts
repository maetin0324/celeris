import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

test("shared rich fixture renders data on the seven task surfaces and Console", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id]")).toHaveCount(24);
    await expect(page.locator("[data-task-id='T1']")).toContainText("複数の画面");

    await page.goto(`${gateway.base}/graph`);
    await expect(page.locator("[data-graph-node]")).toHaveCount(8);
    await expect(page.getByTestId("graph-summary")).toContainText("2 辺");

    await page.goto(`${gateway.base}/tasks/T1`);
    await expect(page.getByTestId("task-overview")).toContainText("長い目的文");
    await expect(page.getByTestId("tree-child")).toHaveCount(2);

    await page.goto(`${gateway.base}/tasks/T1/changes`);
    await expect(page.getByTestId("changed-files").locator("[data-file]")).toHaveCount(15);

    await page.goto(`${gateway.base}/tasks/T1/files`);
    await expect(page.getByTestId("files-tree")).toContainText("long-file-0.tsx");

    await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
    await expect(page.getByTestId("run-log-line-count")).toHaveAttribute("data-count", "27");
    await expect(page.getByTestId("run-log")).toContainText("検証ログ 1");

    await page.goto(`${gateway.base}/artifacts?project=P1`);
    await expect(page.getByTestId("artifacts-list").locator("[data-artifact]")).toHaveCount(12);

    await page.goto(gateway.base);
    await expect(page.locator("[data-console]")).toContainText("画面群の長文");
    await expect(page.locator("[data-console]")).toContainText("read_file");
  } finally {
    await gateway.close();
  }
});
