import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, expectTypeOf, it } from "vitest";
import type { ConfirmDialogProps } from "./confirm-dialog";
import { openOverlayFixture, OVERLAY_FIXTURE_TIMEOUT, seriousViolations } from "./overlay-browser-test";

describe("ConfirmDialog", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  beforeAll(async () => {
    fixture = await openOverlayFixture();
  }, OVERLAY_FIXTURE_TIMEOUT);
  afterAll(async () => {
    await fixture?.close();
  });

  it("確認に必要な対象・結果・復元・追跡を型で要求する", () => {
    expectTypeOf<ConfirmDialogProps>().toMatchTypeOf<{
      title: string;
      target: string;
      consequence: string;
      reversibility: string;
      followUp: string;
    }>();
  });

  it("戻るが初期 focus で、本文・destructive・focus 復帰を保つ", async () => {
    const { page } = fixture;
    await page.getByRole("button", { name: "削除を確認" }).click();
    const dialog = page.getByRole("alertdialog", { name: "タスクを削除" });
    await dialog.waitFor();
    expect(await seriousViolations(page)).toEqual([]);
    expect(await dialog.textContent()).toContain("対象: タスク A。タスク A とその実行履歴を削除します。");
    expect(await dialog.textContent()).toContain("元に戻す方法: 元に戻せません。");
    expect(await dialog.textContent()).toContain("結果の確認: タスク一覧で確認できます。");
    const cancel = dialog.getByRole("button", { name: "戻る" });
    expect(await cancel.evaluate((element) => element === document.activeElement)).toBe(true);
    expect(await dialog.getByRole("button", { name: "タスク A を削除" }).getAttribute("class")).toContain(
      "bg-destructive",
    );
    await cancel.click();
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(() => document.activeElement?.textContent === "削除を確認");
    expect(
      await page.getByRole("button", { name: "削除を確認" }).evaluate((element) => element === document.activeElement),
    ).toBe(true);
  });

  it("送信中の二重送信と Escape を防ぎ、失敗時は dialog にエラーを残す", async () => {
    const { page } = fixture;
    await page.getByRole("button", { name: "削除を確認" }).click();
    const dialog = page.getByRole("alertdialog", { name: "タスクを削除" });
    const confirm = dialog.getByRole("button", { name: "タスク A を削除" });
    await confirm.click();
    expect(await confirm.isDisabled()).toBe(true);
    await confirm.evaluate((element) => (element as HTMLButtonElement).click());
    expect(await dialog.getByRole("status").textContent()).toContain("処理中");
    await page.keyboard.press("Escape");
    expect(await dialog.isVisible()).toBe(true);
    expect(await page.locator("#calls").textContent()).toBe("1");
    await page.evaluate("window.rejectOverlay()");
    await dialog.getByRole("alert").waitFor();
    expect(await dialog.getByRole("alert").textContent()).toContain("削除できませんでした");
    expect(await confirm.isEnabled()).toBe(true);
    await confirm.click();
    expect(await page.locator("#calls").textContent()).toBe("2");
    await page.evaluate("window.resolveOverlay()");
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(() => document.activeElement?.textContent === "削除を確認");
  });

  it("各幅の確認前後を表示確認用に記録できる", async () => {
    const directory = process.env.OVERLAY_SHOTS_DIR;
    if (!directory) return;
    await mkdir(directory, { recursive: true });
    const { page } = fixture;
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await page.screenshot({ path: join(directory, `confirm-before-${width}.png`) });
      await page.getByRole("button", { name: "削除を確認" }).click();
      const dialog = page.getByRole("alertdialog", { name: "タスクを削除" });
      await dialog.waitFor();
      await page.screenshot({ path: join(directory, `confirm-after-${width}.png`) });
      await dialog.getByRole("button", { name: "戻る" }).click();
      await dialog.waitFor({ state: "hidden" });
    }
  }, 30_000);
});
