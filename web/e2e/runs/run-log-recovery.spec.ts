import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// run ログ・Console の取得失敗から、ページを再読み込みせずに画面の「再試行」で戻れること（qa-ops fix-runs-console）。
// 失敗は page.route で返し、flag を下ろしてから押す（時計ではなく出来事で切り替える）。

const timeout = { timeout: 20_000 };

test("run ログの取得失敗は再試行で読み直せる", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    let fail = true;
    await page.route("**/files/tasks/T1/runs/R1/stdout*", (route) =>
      fail ? route.fulfill({ status: 503, body: "unavailable" }) : route.fallback(),
    );
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
    const error = page.locator('[data-fetch-state="error"]');
    await expect(error).toHaveAttribute("role", "alert", timeout);
    await expect(error).toContainText("run ログを取得できません");
    fail = false;
    await error.getByRole("button", { name: "再試行" }).click();
    await expect(error).toHaveCount(0, timeout);
    await expect(page.getByTestId("run-log")).toContainText("検証ログ 1", timeout);
  } finally {
    await gateway.close();
  }
});

test("Console の取得失敗は再試行で読み直せる", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    let fail = true;
    await page.route(/\/api\/console\?/, (route) =>
      fail ? route.fulfill({ status: 503, body: "unavailable" }) : route.fallback(),
    );
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/console`);
    const error = page.locator('[data-fetch-state="error"]');
    await expect(error).toHaveAttribute("role", "alert", timeout);
    fail = false;
    await error.getByRole("button", { name: "再試行" }).click();
    await expect(error).toHaveCount(0, timeout);
    await expect(page.getByRole("list", { name: "Console の会話" })).toContainText("画面群の長文", timeout);
  } finally {
    await gateway.close();
  }
});
