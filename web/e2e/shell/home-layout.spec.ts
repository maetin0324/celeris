/// <reference lib="dom" />
import { expect, type Locator, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// ホーム（/）の初回表示: h1・判断待ちの最初の項目・会話の入力欄が viewport 内にあり、ページは scroll していない。
// 退行の形: 会話の読み込み後に ConsoleView がページを末尾へ送り、上に空白帯が出て h1 と判断待ちが押し出された。
// 偽 daemon（rich profile）と gateway は loopback の空き port。外部ネットワークに出ない。

async function expectInViewport(page: Page, locator: Locator, name: string) {
  const viewport = page.viewportSize();
  if (!viewport) throw new Error("no viewport");
  await expect(locator, name).toBeVisible();
  const b = await locator.boundingBox();
  if (!b) throw new Error(`no box: ${name}`);
  expect(b.y, `${name} の上端`).toBeGreaterThanOrEqual(0);
  expect(b.x, `${name} の左端`).toBeGreaterThanOrEqual(0);
  expect(b.y + b.height, `${name} の下端`).toBeLessThanOrEqual(viewport.height);
  expect(b.x + b.width, `${name} の右端`).toBeLessThanOrEqual(viewport.width);
  return b;
}

for (const [width, height] of [
  [360, 800],
  [1440, 900],
] as const) {
  test(`ホームの初回表示に空白帯が無く、h1・判断待ち・入力欄が見える（${width}x${height}）`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height });
      await page.goto(`${gateway.base}/`);
      const main = page.locator("#main");
      const heading = main.getByRole("heading", { level: 1, name: "ホーム" });
      const first = main
        .getByRole("navigation", { name: "受信箱と通知" })
        .getByRole("list", { name: "期限の近い判断待ち" })
        .getByRole("listitem")
        .first();
      const input = page.getByRole("textbox", { name: "Console への入力" });
      // 会話の block が出た後（ConsoleView の追記の追従が走った後）で測る。
      await expect(main.getByRole("list", { name: "Console の会話" }).getByRole("listitem").first()).toBeVisible();
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));

      const scroll = await page.evaluate(() => ({
        y: window.scrollY,
        top: (document.scrollingElement ?? document.documentElement).scrollTop,
        overflow: (document.scrollingElement ?? document.documentElement).scrollHeight - window.innerHeight,
      }));
      expect(scroll.y, "window の scroll 位置").toBe(0);
      expect(scroll.top, "document の scroll 位置").toBe(0);
      expect(scroll.overflow, "ページが viewport より高くない").toBeLessThanOrEqual(1);

      const h1 = await expectInViewport(page, heading, "h1『ホーム』");
      // 空白帯が無い: h1 は shell の上帯（360 では header）と main の余白のすぐ下にある。
      expect(h1.y, "h1 の上端").toBeLessThan(width < 768 ? 120 : 60);
      await expectInViewport(page, first, "判断待ちの最初の項目");
      await expectInViewport(page, input, "会話の入力欄");
    } finally {
      await gateway.close();
    }
  });
}

// 会話は枠の中で scroll する。開いた直後は枠の末尾にいて、追記に合わせて末尾へ送る。
// 枠の上へ離れている間は追記で動かさず「最新へ」を出す。どの間もページ（window）は scroll しない。
test("ホームの会話の枠: 追記の追従と『最新へ』（ページは動かない）", async ({ page }) => {
  const human = (i: number) => ({
    kind: "human",
    at: "2026-01-01T00:00:00Z",
    cursor: `f${i}`,
    message_id: `hf${i}`,
    node_id: "cos",
    text: `発言 ${i}`,
  });
  const gateway = await startFixtureGateway({
    fixtures: { "/api/v1/console": { items: Array.from({ length: 30 }, (_, i) => human(i)), next_cursor: "n0" } },
  });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/`);
    await expect(page.getByText("発言 29")).toBeVisible();
    await expect.poll(() => gateway.daemon.consoleClients).toBeGreaterThan(0);
    const region = page.locator("[data-home-console]");
    const gap = () => region.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
    const windowY = () => page.evaluate(() => window.scrollY);
    const input = page.getByRole("textbox", { name: "Console への入力" });
    // 開いた直後は枠の末尾（最後の発言が送信欄の上に見える）。h1 は見えたまま。
    await expect.poll(gap).toBeLessThanOrEqual(200);
    await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeInViewport();
    const last = await page.getByText("発言 29").boundingBox();
    const inputBox = await input.boundingBox();
    if (!last || !inputBox) throw new Error("no box");
    expect(last.y + last.height).toBeLessThanOrEqual(inputBox.y);
    expect(await windowY()).toBe(0);
    // 上へ離れている間は追記で動かさず、「最新へ」を出す。
    await region.evaluate((el) => el.scrollTo(0, 0));
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    gateway.daemon.sendConsoleBlock({ ...human(30), text: "追記 1" });
    const latest = page.getByRole("button", { name: "最新へ" });
    await expect(latest).toBeVisible();
    await expect(latest).toBeInViewport();
    expect(await region.evaluate((el) => el.scrollTop)).toBe(0);
    await latest.click();
    await expect(page.getByText("追記 1")).toBeInViewport();
    await expect(latest).toBeHidden();
    // 末尾にいれば追記に合わせて末尾へ送る。
    gateway.daemon.sendConsoleBlock({ ...human(31), text: "追記 2" });
    await expect(page.getByText("追記 2")).toBeInViewport();
    expect(await windowY()).toBe(0);
  } finally {
    await gateway.close();
  }
});
