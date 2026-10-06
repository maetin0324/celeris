import { expect, test } from "@playwright/test";
import { expectInFirstScreen, FIRST_SCREEN_SIZES, waitForScreen } from "../support/first-screen";
import { startFixtureGateway } from "../support/fixture-gateway";

// 縦の長さの gate（タスク詳細・run ログ）: 目的の中身が scroll 前の最初の 1 画面に入る（狭い幅と広い幅）。
// /tasks/T1 は目的の本文と状態、/tasks/T1/runs/R1 はログ本文の面の先頭。fixture（偽 daemon の rich）で測る。
// あわせて ?tab= で開いたときにその tab が選ばれていること（URL からの復元）を見る。

const OBJECTIVE_HEAD = "長い目的文を複数行で表示し、親子関係・依存関係・作業ツリーと成果物を確認する。";

for (const size of FIRST_SCREEN_SIZES) {
  test(`/tasks/T1 ${size.width}: 目的の本文と状態が最初の 1 画面に入る`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize(size);
      await page.goto(`${gateway.base}/tasks/T1`);
      await waitForScreen(page, "タスクの詳細 T1");
      await expect(page.locator("[data-tab='overview']")).toHaveAttribute("aria-current", "page");
      const objective = page.getByTestId("task-objective");
      await expect(objective).toContainText(OBJECTIVE_HEAD);
      await expectInFirstScreen(page, objective, "目的の本文");
      await expectInFirstScreen(
        page,
        page.getByTestId("task-header-status").locator("[data-status]"),
        "状態（StatusBadge）",
        20,
      );
      // 題はヘッダーに全文で 1 回だけ出す（概要で再掲しない）。
      const title = await page.getByTestId("task-header-title").innerText();
      expect(title.length).toBeGreaterThan(0);
      await expect(page.getByTestId("task-overview").getByRole("heading", { name: title })).toHaveCount(0);
    } finally {
      await gateway.close();
    }
  });

  test(`/tasks/T1/runs/R1 ${size.width}: ログ本文の先頭が最初の 1 画面に入る`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize(size);
      await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
      await waitForScreen(page, "run ログ T1 / R1");
      await expect(page.getByTestId("run-header")).toBeVisible();
      const body = page.getByRole("region", { name: "run ログ本文" });
      await expect(body.locator("li").first()).toBeVisible();
      await expectInFirstScreen(page, body, "ログ本文の面");
      await expectInFirstScreen(page, page.getByTestId("run-header"), "run の概要", 20);
    } finally {
      await gateway.close();
    }
  });
}

test("/tasks/T1?tab= で開くとその tab が選ばれている（URL から復元）", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    for (const tab of ["timeline", "changes", "files", "artifacts"]) {
      await page.goto(`${gateway.base}/tasks/T1?tab=${tab}`);
      await waitForScreen(page, "タスクの詳細 T1");
      await expect(page.locator(`[data-tab='${tab}']`)).toHaveAttribute("aria-current", "page");
      await expect(page.locator("[data-tab='overview']")).not.toHaveAttribute("aria-current", "page");
      await expect(page.getByTestId("task-overview")).toHaveCount(0);
    }
    await page.goto(`${gateway.base}/tasks/T1?tab=overview`);
    await waitForScreen(page, "タスクの詳細 T1");
    await expect(page.locator("[data-tab='overview']")).toHaveAttribute("aria-current", "page");
    await expect(page.getByTestId("task-objective")).toBeVisible();
  } finally {
    await gateway.close();
  }
});
