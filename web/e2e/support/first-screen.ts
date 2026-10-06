import { expect, type Locator, type Page } from "@playwright/test";

// 縦の長さの gate（2026-10-06 の要望）: 目的の中身（選んだ知識の本文、一覧の先頭行など）が、scroll 前の
// 最初の 1 画面に入っていることを狭い幅と広い幅で確かめる。狭い幅では画面下の固定タブ（mobile-tabbar）が
// 覆う帯を除いて測る。fixture（偽 daemon）で決定的に測り、固定の時間では待たない。
export const FIRST_SCREEN_SIZES = [
  { width: 360, height: 800 },
  { width: 1440, height: 800 },
] as const;

const LOADING_SELECTOR = '[data-fetch-state="loading"], [aria-busy="true"]';

/** 見出しが出て、読み込み中（FetchFrame の loading・aria-busy）が消えるまで待つ。 */
export async function waitForScreen(page: Page, heading: string | RegExp) {
  await expect(page.getByRole("heading", { level: 1, name: heading })).toBeVisible();
  await page.waitForFunction((selector) => !document.querySelector(selector), LOADING_SELECTOR);
}

/** scroll 前の viewport に `locator` の先頭 `minVisible` px が入っている（下部タブに隠れる帯は含めない）。 */
export async function expectInFirstScreen(page: Page, locator: Locator, name: string, minVisible = 48) {
  const viewport = page.viewportSize();
  if (!viewport) throw new Error("no viewport");
  expect(await page.evaluate(() => window.scrollY), `${name}: scroll 前に測る`).toBe(0);
  await expect(locator, name).toBeVisible();
  const box = await locator.boundingBox();
  if (!box) throw new Error(`no box: ${name}`);
  const tabbar = await page.locator('[data-testid="mobile-tabbar"]').boundingBox();
  const bottom = tabbar ? Math.min(viewport.height, tabbar.y) : viewport.height;
  expect(box.y, `${name} の上端`).toBeGreaterThanOrEqual(0);
  expect(box.y + Math.min(box.height, minVisible), `${name} の見える下端（${bottom}px まで）`).toBeLessThanOrEqual(
    bottom,
  );
  return box;
}
