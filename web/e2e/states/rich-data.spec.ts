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

test("default fixture shows reports, approvals and the review-pending task without fetch errors", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 390, height: 800 });
    await page.goto(`${gateway.base}/reports?report=RP1`);
    await expect(page.locator("h1")).toContainText("報告");
    await expect(page.locator("[data-report-id]")).toHaveCount(3);
    await expect(page.locator("[data-fetch-state='loading']")).toHaveCount(0);
    await expect(page.locator("[data-fetch-state='error']")).toHaveCount(0);
    await expect(page.locator("[data-report-id='RP1']")).toContainText("画面の検証結果を報告します");
    await expect(page.getByText("360・390・412・1440 px の撮影を確認しました。")).toBeVisible();

    await page.goto(`${gateway.base}/approvals`);
    await expect(page.locator("h1")).toContainText("承認");
    await expect(page.locator("[data-fetch-state='loading']")).toHaveCount(0);
    await expect(page.locator("[data-fetch-state='error']")).toHaveCount(0);
    await expect(page.locator("[data-approval-id]")).toHaveCount(2);
    await expect(page.locator("[data-approval-id]").first()).toContainText("web の画像を書き出してよいですか");
    await expect(page.locator("[data-approval-id]").last()).toContainText("ビルドの検査を走らせてよいですか");

    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${gateway.base}/tasks/T1`);
    await expect(page.locator("h1")).toContainText("タスクの詳細 T1");
    await expect(page.locator("[data-fetch-state='loading']")).toHaveCount(0);
    await expect(page.locator("[data-fetch-state='error']")).toHaveCount(0);
    await expect(page.getByTestId("execution-view").locator("[data-phase]")).toHaveAttribute("data-phase", "verifying");
    await expect(page.getByTestId("execution-view")).toContainText("v1（2 件）");
    await expect(page.getByTestId("routing-panel")).toContainText("担当 ui-ux");
    await expect(page.getByTestId("routing-panel")).toContainText("R1: standard / standard / ui-ux（rule-standard）");
  } finally {
    await gateway.close();
  }
});
