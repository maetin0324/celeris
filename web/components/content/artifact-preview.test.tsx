import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { openOverlayFixture, OVERLAY_FIXTURE_TIMEOUT, seriousViolations } from "../ui/overlay-browser-test";

describe("ArtifactPreview", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  beforeAll(async () => {
    fixture = await openOverlayFixture("/components/content/fixtures/preview.html");
  }, OVERLAY_FIXTURE_TIMEOUT);
  afterAll(async () => {
    await fixture?.close();
  });

  it("取得失敗から再取得し、閉じて開き直した後も本文を回復できる", async () => {
    const { page } = fixture;
    await page.setViewportSize({ width: 360, height: 800 });
    let fail = true;
    let requests = 0;
    await page.route("**/files/tasks/T1/artifacts/0", (route) => {
      requests += 1;
      return route.fulfill({ status: fail ? 503 : 200, body: fail ? "unavailable" : "取得した本文" });
    });
    await page.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByRole("alert").waitFor();
    fail = false;
    await page.getByRole("button", { name: "再試行", exact: true }).click();
    await page.getByText("取得した本文", { exact: true }).waitFor();
    expect(await page.getByRole("alert").count()).toBe(0);
    expect(requests).toBe(2);
    await page.getByRole("button", { name: "本文を閉じる", exact: true }).click();
    fail = true;
    await page.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByRole("alert").waitFor();
    await page.getByRole("button", { name: "本文を閉じる", exact: true }).click();
    fail = false;
    await page.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByText("取得した本文", { exact: true }).waitFor();
    expect(await page.getByRole("alert").count()).toBe(0);
  });

  it("4幅で download の44px targetと本文の折り返しを保つ", async () => {
    const { page } = fixture;
    const directory = process.env.PREVIEW_SHOTS_DIR;
    if (directory) await mkdir(directory, { recursive: true });
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 800 });
      const link = page.getByRole("link", { name: "ダウンロード", exact: true });
      const bounds = await link.boundingBox();
      expect(bounds?.width).toBeGreaterThanOrEqual(44);
      expect(bounds?.height).toBeGreaterThanOrEqual(44);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(await seriousViolations(page)).toEqual([]);
      if (directory) await page.screenshot({ path: join(directory, `preview-${width}.png`) });
    }
  });
});
