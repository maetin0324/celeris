import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

test("受信箱の browser_wait から run へ、task 詳細から Live View と待ちの回答へ進める", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  const backend = gateway.daemon.browser;
  if (!backend) throw new Error("browser backend is not enabled");
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/inbox`);
    const item = page.locator('[data-inbox-item="browser_wait:W1"]');
    await expect(item).toBeVisible();
    await expect(item.getByText("ブラウザの承認待ち", { exact: true })).toBeVisible();
    await item.getByRole("link", { name: "ブラウザの実行画面で操作する" }).click();
    await expect(page).toHaveURL(`${gateway.base}/browser/runs/T1/R1#browser-waits`);
    await expect(page.getByTestId("browser-run-screen")).toBeVisible();

    await page.goto(`${gateway.base}/tasks/T1`);
    const section = page.getByTestId("task-browser-section");
    await expect(section).toBeVisible();
    await expect(section.locator('[data-browser-run="R1"]')).toContainText("待ち 1 件");
    await section.locator('[data-browser-run="R1"]').getByRole("link", { name: "Live View を開く" }).click();
    await expect(page).toHaveURL(`${gateway.base}/browser/runs/T1/R1`);
    await expect(page.getByTestId("browser-live-iframe")).toHaveAttribute("src", "/browser/live/T1/R1");

    await page.goto(`${gateway.base}/tasks/T1#browser-waits`);
    await expect(page.getByTestId("task-browser-section").getByTestId("browser-decision-form")).toBeVisible();
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/tasks/T1`);
    await page.getByRole("link", { name: "ブラウザの待ち 1 件に対応" }).click();
    await expect(page).toHaveURL(`${gateway.base}/tasks/T1#browser-waits`);
    await expect(page.getByTestId("task-browser-section").getByTestId("browser-decision-form")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(0);
    const target = await page
      .getByTestId("task-browser-section")
      .getByRole("button", { name: "一回だけ承認" })
      .boundingBox();
    expect(target?.height ?? 0).toBeGreaterThanOrEqual(44);
    expect(target?.width ?? 0).toBeGreaterThanOrEqual(44);
    await page.getByTestId("task-browser-section").getByRole("button", { name: "一回だけ承認" }).click();
    await expect.poll(() => backend.records.waits.length).toBe(1);
    expect(backend.records.waits[0]).toMatchObject({
      task_id: "T1",
      wait_id: "W1",
      kind: "decision",
      body: { decision: "approve_once" },
    });
  } finally {
    await gateway.close();
  }
});

test("承認画面にも browser wait の実行画面への導線がある", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/approvals`);
    await page.getByRole("link", { name: "ブラウザの実行画面を開く" }).click();
    await expect(page).toHaveURL(`${gateway.base}/browser/runs/T1/R1#browser-waits`);
  } finally {
    await gateway.close();
  }
});
