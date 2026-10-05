/// <reference lib="dom" />
import { expect, test } from "@playwright/test";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// shell の mount（P2-02）。偽 daemon を止める → 起こす、の間で shell の DOM の node が入れ替わらない。
// 偽 daemon と gateway は loopback の空き port。
test("shell は daemon の停止と復帰で入れ替わらない", async ({ page }) => {
  let daemon = createFakeDaemon();
  const daemonUrl = await daemon.start();
  const port = Number(new URL(daemonUrl).port);
  const gateway = await startGateway({ daemonUrl });
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    await page.evaluate(() => {
      const w = window as unknown as { __shell?: Element | null; __main?: Element | null };
      w.__shell = document.querySelector("[data-shell]");
      w.__main = document.querySelector("#main");
    });
    const same = () =>
      page.evaluate(() => {
        const w = window as unknown as { __shell?: Element | null; __main?: Element | null };
        return (
          !!w.__shell?.isConnected &&
          w.__shell === document.querySelector("[data-shell]") &&
          w.__main === document.querySelector("#main")
        );
      });

    await daemon.close();
    await page.getByRole("navigation", { name: "主要" }).getByRole("link", { name: "案件" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "案件" })).toBeVisible();
    await page.waitForTimeout(500);
    expect(await same()).toBe(true);

    daemon = createFakeDaemon({ port });
    await daemon.start();
    await page.getByRole("navigation", { name: "主要" }).getByRole("link", { name: "タスク" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    await page.waitForTimeout(500);
    expect(await same()).toBe(true);
  } finally {
    await gateway.close();
    await daemon.close();
  }
});

test("遷移後に見出しへ focus、戻るでスクロール位置が戻る", async ({ page }) => {
  const gateway = await startGateway();
  try {
    await page.setViewportSize({ width: 390, height: 700 });
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    // 枠だけの画面は短いので、root の外に高さを足してスクロールできるようにする。
    await page.evaluate(() => {
      const spacer = document.createElement("div");
      spacer.style.height = "4000px";
      document.body.append(spacer);
      window.scrollTo(0, 1200);
    });
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(1200);
    // playwright の click は対象を画面に入れるため scroll を変える。DOM の click で scroll を保ったまま移る。
    await page.evaluate(() => document.querySelector<HTMLElement>('#shell-nav a[href="/projects"]')?.click());
    await expect(page).toHaveURL(`${gateway.base}/projects`);
    await expect(page.getByRole("heading", { level: 1, name: "案件" })).toBeFocused();
    await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0);
    await page.goBack();
    await expect(page).toHaveURL(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    await expect.poll(() => page.evaluate(() => Math.abs(window.scrollY - 1200))).toBeLessThanOrEqual(20);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeFocused();
  } finally {
    await gateway.close();
  }
});
