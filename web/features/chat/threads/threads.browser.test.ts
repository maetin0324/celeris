import { describe, expect, it } from "vitest";
import { OVERLAY_FIXTURE_TIMEOUT, openOverlayFixture } from "../../../components/ui/overlay-browser-test";

describe("chat threads browser", () => {
  it(
    "chat_threads_mobile_drawer_restores_focus_on_escape_and_selection",
    async () => {
      const fixture = await openOverlayFixture("/features/chat/threads/fixtures/threads.html");
      try {
        const { page } = fixture;
        await page.setViewportSize({ width: 360, height: 700 });
        const trigger = page.getByRole("button", { name: "会話一覧を開く" });
        await trigger.click();
        const dialog = page.getByRole("dialog", { name: "会話一覧" });
        await dialog.waitFor();
        await page.keyboard.press("Escape");
        await dialog.waitFor({ state: "hidden" });
        await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "会話一覧を開く");
        expect(await trigger.evaluate((element) => element === document.activeElement)).toBe(true);
        await trigger.click();
        await dialog.getByRole("button", { name: "作業の相談", exact: true }).click();
        await dialog.waitFor({ state: "hidden" });
        expect(await page.locator("body").getAttribute("data-selected")).toBe("human");
        await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "会話一覧を開く");
      } finally {
        await fixture.close();
      }
    },
    OVERLAY_FIXTURE_TIMEOUT,
  );
});
