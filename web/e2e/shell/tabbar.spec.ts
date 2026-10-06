/// <reference lib="dom" />
import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// md 未満の画面下の固定タブ（旧 GUI の MobileTabBar と同等）。主要 4 つ（ホーム・受信箱・タスク・案件）と「その他」。
// 「その他」は残りの項目を下からのシート（Radix Dialog）で出す。md 以上は出さず、側面の nav のまま。
// 偽 daemon（rich profile。受信箱 5 件・未読の通知 3 件）と gateway は loopback の空き port。外部ネットワークに出ない。
let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  gateway = await startFixtureGateway();
});
test.afterAll(async () => {
  await gateway.close();
});

const TABS = ["ホーム", "受信箱", "タスク", "案件"] as const;

for (const width of [360, 390, 412]) {
  test(`タブ: 4 つの link と「その他」が画面下に固定され、44px 以上で、現在地を示す（${width}px）`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    const bar = page.getByTestId("mobile-tabbar");
    await expect(bar).toBeVisible();
    const nav = page.getByRole("navigation", { name: "主要（モバイル）" });
    await expect(nav).toBeVisible();
    await expect(nav.getByRole("link")).toHaveCount(4);
    const more = nav.getByRole("button", { name: "その他" });
    await expect(more).toBeVisible();
    await expect(more).toHaveAttribute("aria-expanded", "false");

    for (const control of [...TABS.map((name) => nav.getByRole("link", { name })), more]) {
      const b = await control.boundingBox();
      expect(b?.width ?? 0).toBeGreaterThanOrEqual(44);
      expect(b?.height ?? 0).toBeGreaterThanOrEqual(44);
    }
    // 文字は 1 行（360 でも折り返さない）。
    const lines = await nav
      .locator("a > span:not([class*='absolute']), button > span:not([aria-hidden])")
      .evaluateAll((spans) => spans.map((span) => span.getClientRects().length));
    expect(lines.length).toBe(5);
    for (const count of lines) expect(count).toBe(1);

    const box = await bar.boundingBox();
    if (!box) throw new Error("no box: mobile-tabbar");
    expect(Math.abs(box.y + box.height - 800)).toBeLessThanOrEqual(0.5);
    expect(box.x).toBe(0);
    expect(box.width).toBe(width);

    await expect(nav.getByRole("link", { name: "タスク" })).toHaveAttribute("aria-current", "page");
    for (const name of ["ホーム", "受信箱", "案件"])
      await expect(nav.getByRole("link", { name })).not.toHaveAttribute("aria-current", /.*/);
    await expect(more).not.toHaveAttribute("data-active", "true");
    // ページ全体に横スクロールを作らない。
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}

test("タブ: 受信箱の件数を badge で出し、未読の通知は「その他」の点で知らせる", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto(`${gateway.base}/tasks`);
  const nav = page.getByRole("navigation", { name: "主要（モバイル）" });
  const inbox = nav.getByRole("link", { name: /受信箱/ });
  await expect(inbox.locator("[data-badge]")).toHaveText("5");
  await expect(inbox.getByRole("img", { name: "受信箱 5 件" })).toBeVisible();
  const more = nav.getByRole("button", { name: "その他" });
  await expect(more.locator('span[aria-hidden="true"].rounded-full')).toBeVisible();
  await more.click();
  const sheet = page.getByRole("dialog", { name: "その他" });
  await expect(sheet.getByRole("link", { name: /通知/ }).getByRole("img", { name: "未読の通知 3 件" })).toBeVisible();
});

test("その他: シートが開き、Escape で閉じて focus を戻す。知識へ移ると閉じて「その他」が現在地になる", async ({
  page,
}) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto(`${gateway.base}/tasks`);
  await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
  const nav = page.getByRole("navigation", { name: "主要（モバイル）" });
  const more = nav.getByRole("button", { name: "その他" });
  await more.click();
  const sheet = page.getByRole("dialog", { name: "その他" });
  await expect(sheet).toBeVisible();
  await expect(sheet).toHaveAttribute("aria-modal", "true");
  // 開いている間は背後が aria-hidden になるので、role ではなく testid から「その他」を引く。
  await expect(page.getByTestId("mobile-tabbar").locator("button")).toHaveAttribute("aria-expanded", "true");
  await expect(sheet.getByRole("list", { name: "日々の仕事" })).toBeVisible();
  await expect(sheet.getByRole("list", { name: "組織と管理" })).toBeVisible();
  // 主要 4 つはシートに重ねて出さない。
  for (const name of TABS) await expect(sheet.getByRole("link", { name, exact: true })).toHaveCount(0);
  await expect(sheet.getByRole("link", { name: "知識" })).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();
  await expect(more).toHaveAttribute("aria-expanded", "false");
  await expect(more).toBeFocused();

  await more.click();
  await page.getByRole("dialog", { name: "その他" }).getByRole("link", { name: "知識" }).click();
  await expect(page).toHaveURL(`${gateway.base}/knowledge`);
  await expect(page.getByRole("dialog", { name: "その他" })).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 1, name: "知識" })).toBeFocused();
  await expect(more).toHaveAttribute("data-active", "true");
  for (const name of TABS) await expect(nav.getByRole("link", { name })).not.toHaveAttribute("aria-current", /.*/);
  // 現在地が「その他」の中にあるときは未読の点を出さない。
  await expect(more.locator('span[aria-hidden="true"].rounded-full')).toHaveCount(0);
});

test("タブ: Tab で 4 つの link と「その他」を順に辿り、Enter で移る", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto(`${gateway.base}/tasks`);
  await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
  const nav = page.getByRole("navigation", { name: "主要（モバイル）" });
  const home = nav.getByRole("link", { name: "ホーム" });
  await home.focus();
  await expect(home).toBeFocused();
  for (const name of TABS.slice(1)) {
    await page.keyboard.press("Tab");
    await expect(nav.getByRole("link", { name })).toBeFocused();
  }
  await page.keyboard.press("Tab");
  await expect(nav.getByRole("button", { name: "その他" })).toBeFocused();
  expect(await nav.getByRole("button", { name: "その他" }).evaluate((el) => el.matches(":focus-visible"))).toBe(true);

  await page.keyboard.press("Shift+Tab");
  await expect(nav.getByRole("link", { name: "案件" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(`${gateway.base}/projects`);
  await expect(page.getByRole("heading", { level: 1, name: "案件" })).toBeFocused();
  await expect(nav.getByRole("link", { name: "案件" })).toHaveAttribute("aria-current", "page");
});

test("タブ: 長い画面を末尾まで scroll しても本文の最後がタブに隠れない（360px）", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto(`${gateway.base}/help`);
  await expect(page.getByRole("heading", { level: 1, name: "ヘルプ" })).toBeVisible();
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const result = await page.evaluate(() => {
    const main = document.querySelector("#main");
    const bar = document.querySelector('[data-testid="mobile-tabbar"]');
    const slot = document.querySelector("aside[data-console-slot]");
    if (!main || !bar || !slot) throw new Error("no main / tabbar / console slot");
    // main の子孫のうち一番下に描かれた要素。
    let bottom = 0;
    for (const el of Array.from(main.querySelectorAll("*"))) {
      const box = el.getBoundingClientRect();
      if (box.height > 0) bottom = Math.max(bottom, box.bottom);
    }
    return {
      scrolled: window.scrollY > 0,
      bottom,
      slot: slot.getBoundingClientRect().bottom,
      barTop: bar.getBoundingClientRect().top,
    };
  });
  expect(result.scrolled).toBe(true);
  expect(result.bottom).toBeLessThanOrEqual(result.barTop);
  expect(result.slot).toBeLessThanOrEqual(result.barTop + 0.5);
});

test("タブ: md 以上は出さず、側面の nav のまま（1440px）", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${gateway.base}/tasks`);
  await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
  await expect(page.getByTestId("mobile-tabbar")).toBeHidden();
  await expect(page.getByRole("navigation", { name: "主要", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "メニュー" })).toBeHidden();
  // 本文の列に下の余白を足さない。
  const pad = await page.evaluate(
    () => getComputedStyle(document.querySelector("#main")?.parentElement as HTMLElement).paddingBottom,
  );
  expect(pad).toBe("0px");
});
