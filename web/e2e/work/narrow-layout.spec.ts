import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// fix-r5 fix-narrow: 360/390 の目視指摘を固定する（rich fixture、screenshots と同じ data）。
// (a) /artifacts 360: タスクの題名が細い列で 20 行近く折れていた → 題名は 2 行以下、全文は title 属性。
// (b) /projects/P1 360・390: 『途中目標と仕事』の状態・判断待ちが右で切れていた → 行を積み、表が枠の幅に収まる。

test("/artifacts 360: task title stays within two lines and keeps the full title in title", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/artifacts?project=P1`);
    const list = page.getByTestId("artifacts-list");
    await expect(list.locator("[data-artifact]").first()).toBeVisible();
    const titles = list.getByTestId("artifact-task-title").filter({ visible: true });
    await expect(titles.first()).toBeVisible();
    const count = await titles.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i += 1) {
      const title = titles.nth(i);
      const metrics = await title.evaluate((el) => {
        const text = el.firstElementChild as HTMLElement;
        const lineHeight = Number.parseFloat(getComputedStyle(text).lineHeight);
        return { height: text.getBoundingClientRect().height, lineHeight, width: el.getBoundingClientRect().width };
      });
      // 2 行分以下（丸めの 2px を許す）。題名は表の幅の大半を使う（1/4 に潰れない）。
      expect(metrics.height).toBeLessThanOrEqual(metrics.lineHeight * 2 + 2);
      expect(metrics.width).toBeGreaterThan(360 / 2);
      await expect(title).toHaveAttribute("title", /\S/);
    }
    // 長い題名（T1）は省略されても title に全文がある。
    await expect(titles.filter({ hasText: "複数の画面" }).first()).toHaveAttribute(
      "title",
      /複数の画面にまたがる長いタスク名と依存関係を確認する作業 複数の画面/,
    );
  } finally {
    await gateway.close();
  }
});

for (const width of [360, 390]) {
  test(`/projects/P1 ${width}: the work table fits the viewport and stacks status and pending`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/projects/P1`);
      const frame = page.getByTestId("project-tree-frame");
      await expect(frame.locator("[data-tree-task='T1']")).toBeVisible();
      const box = await frame.evaluate((el) => {
        const table = el.querySelector("table") as HTMLTableElement;
        const rect = el.getBoundingClientRect();
        return {
          right: rect.right,
          scrollWidth: el.scrollWidth,
          clientWidth: el.clientWidth,
          tableRight: table.getBoundingClientRect().right,
        };
      });
      // 表の右端が viewport 内にあり、枠の中でも横に溢れない（状態・判断待ちが切れない）。
      expect(box.right).toBeLessThanOrEqual(width);
      expect(box.tableRight).toBeLessThanOrEqual(width);
      expect(box.scrollWidth).toBeLessThanOrEqual(box.clientWidth + 1);
      // 各行の状態は「状態:」の見出し付きで、viewport の中に見える。
      const rows = frame.locator("[data-tree-task]");
      const count = await rows.count();
      expect(count).toBeGreaterThan(0);
      for (let i = 0; i < count; i += 1) {
        const row = rows.nth(i);
        await expect(row.getByText("状態:")).toBeVisible();
        await expect(row.getByText(/判断待ち/).first()).toBeVisible();
        const status = row.locator("[data-status]");
        await expect(status).toBeInViewport();
        const right = await status.evaluate((el) => el.getBoundingClientRect().right);
        expect(right).toBeLessThanOrEqual(width);
      }
    } finally {
      await gateway.close();
    }
  });
}

test("/projects/P1 1280: the work table keeps the three columns", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${gateway.base}/projects/P1`);
    const frame = page.getByTestId("project-tree-frame");
    await expect(frame.getByRole("columnheader", { name: "状態" })).toBeVisible();
    await expect(frame.getByRole("columnheader", { name: "判断待ち" })).toBeVisible();
    await expect(frame.locator("[data-tree-task='T1']").getByText("状態:")).toBeHidden();
  } finally {
    await gateway.close();
  }
});
