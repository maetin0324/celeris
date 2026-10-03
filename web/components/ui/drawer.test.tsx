import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, expectTypeOf, it } from "vitest";
import type { DrawerProps } from "./drawer";
import { openOverlayFixture, seriousViolations } from "./overlay-browser-test";

describe("Drawer / SidePanel", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  beforeAll(async () => {
    fixture = await openOverlayFixture();
  }, 30_000);
  afterAll(async () => {
    await fixture?.close();
  });

  it("title と focus 復帰用 trigger を型で要求する", () => {
    expectTypeOf<DrawerProps>().toMatchTypeOf<{ title: string; trigger: React.ReactElement }>();
  });

  it("360px では全幅、Escape で閉じ trigger に focus を戻す", async () => {
    const { page } = fixture;
    await page.setViewportSize({ width: 360, height: 700 });
    await page.getByRole("button", { name: "詳細を開く" }).click();
    const dialog = page.getByRole("dialog", { name: "タスクの詳細" });
    await dialog.waitFor();
    expect(await seriousViolations(page)).toEqual([]);
    expect(await dialog.getAttribute("aria-describedby")).toBeTruthy();
    const box = await dialog.boundingBox();
    expect(box?.width).toBe(360);
    const close = dialog.getByRole("button", { name: "閉じる" });
    const closeBox = await close.boundingBox();
    expect(closeBox?.width).toBeGreaterThanOrEqual(44);
    expect(closeBox?.height).toBeGreaterThanOrEqual(44);
    await close.focus();
    await page.keyboard.press("Tab");
    expect(await page.evaluate(() => document.querySelector('[role="dialog"]')?.contains(document.activeElement))).toBe(
      true,
    );
    await page.keyboard.press("Shift+Tab");
    expect(await page.evaluate(() => document.querySelector('[role="dialog"]')?.contains(document.activeElement))).toBe(
      true,
    );
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(() => document.activeElement?.textContent === "詳細を開く");
    expect(
      await page.getByRole("button", { name: "詳細を開く" }).evaluate((element) => element === document.activeElement),
    ).toBe(true);
  });

  it("desktop では drawer token の上限に収まり、reduced motion で遷移しない", async () => {
    const { page } = fixture;
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.getByRole("button", { name: "詳細を開く" }).click();
    const dialog = page.getByRole("dialog", { name: "タスクの詳細" });
    await dialog.waitFor();
    const box = await dialog.boundingBox();
    expect(box?.width).toBe(512);
    expect(
      Number.parseFloat(await dialog.evaluate((element) => getComputedStyle(element).transitionDuration)),
    ).toBeLessThan(0.001);
    await dialog.getByRole("button", { name: "閉じる" }).click();
  });

  it("各幅の開閉前後を表示確認用に記録できる", async () => {
    const directory = process.env.OVERLAY_SHOTS_DIR;
    if (!directory) return;
    await mkdir(directory, { recursive: true });
    const { page } = fixture;
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await page.screenshot({ path: join(directory, `drawer-before-${width}.png`) });
      await page.getByRole("button", { name: "詳細を開く" }).click();
      const dialog = page.getByRole("dialog", { name: "タスクの詳細" });
      await dialog.waitFor();
      await page.screenshot({ path: join(directory, `drawer-after-${width}.png`) });
      await dialog.getByRole("button", { name: "閉じる" }).click();
      await dialog.waitFor({ state: "hidden" });
    }
  }, 30_000);
});
