import { expect, test } from "@playwright/test";
import { expectInFirstScreen, FIRST_SCREEN_SIZES, waitForScreen } from "../support/first-screen";
import { startFixtureGateway } from "../support/fixture-gateway";

// 知識・手順書の画面（2026-10-06 の要望）: 選んだ知識の本文・手順書の本文・一覧の先頭行が、scroll 前の
// 最初の 1 画面に入る。広い幅は一覧と中身の 2 ペイン、狭い幅は中身を見出しの直下に出す。
let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  gateway = await startFixtureGateway();
});
test.afterAll(async () => {
  await gateway.close();
});

for (const size of FIRST_SCREEN_SIZES) {
  test(`/knowledge?path= ${size.width}: 本文が最初の 1 画面に入り、選んだ行が強調される`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${gateway.base}/knowledge?path=projects%2Fdemo.md`);
    await waitForScreen(page, "知識");
    await expectInFirstScreen(page, page.getByText("Knowledge body"), "本文");
    await expect(page.getByRole("heading", { level: 2, name: "Demo knowledge" })).toBeVisible();
    // 選んだ行は aria-current（狭い幅では一覧を畳むので、DOM の属性で確かめる）。
    const row = page.locator('a[aria-current="true"]', { hasText: "Demo knowledge" });
    await expect(row).toHaveCount(1);
    if (size.width >= 1024) await expect(row).toBeVisible();
  });

  test(`/knowledge ${size.width}: 先頭の結果行が最初の 1 画面に入る`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${gateway.base}/knowledge`);
    await waitForScreen(page, "知識");
    await expectInFirstScreen(page, page.getByRole("link", { name: "Demo knowledge" }), "先頭の結果行", 44);
  });

  test(`/knowledge/skills?name= ${size.width}: 手順書の本文が最初の 1 画面に入る`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${gateway.base}/knowledge/skills?name=demo`);
    await waitForScreen(page, "手順書（skills）");
    await expectInFirstScreen(page, page.getByText("Description", { exact: true }), "手順書の本文", 20);
  });
}

test("/knowledge 1440: 一覧の行を選ぶと URL に path が入り、右ペインに本文が出る（scroll は 0 のまま）", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 800 });
  await page.goto(`${gateway.base}/knowledge`);
  await waitForScreen(page, "知識");
  await page.getByRole("link", { name: "Demo knowledge" }).click();
  await expect(page).toHaveURL(/path=projects%2Fdemo.md/);
  const body = page.getByText("Knowledge body");
  await expect(body).toBeVisible();
  const row = page.getByRole("link", { name: "Demo knowledge" });
  await expect(row).toHaveAttribute("aria-current", "true");
  const [rowBox, bodyBox] = [await row.boundingBox(), await body.boundingBox()];
  if (!rowBox || !bodyBox) throw new Error("no box");
  expect(bodyBox.x, "本文は一覧の右のペイン").toBeGreaterThan(rowBox.x + rowBox.width);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
});

test("/knowledge 360: 一覧の行を選ぶと中身の見出しへ focus が移る", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${gateway.base}/knowledge`);
  await waitForScreen(page, "知識");
  await page.getByRole("link", { name: "Demo knowledge" }).click();
  await expect(page).toHaveURL(/path=projects%2Fdemo.md/);
  await expect(page.getByRole("heading", { level: 2, name: "Demo knowledge" })).toBeFocused();
  await expect(page.getByRole("link", { name: "一覧に戻る" })).toBeVisible();
  await expectInFirstScreen(page, page.getByText("Knowledge body"), "本文");
});
