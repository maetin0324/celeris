/// <reference lib="dom" />
import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// shell の作り直し: page header（パンくず・説明・主操作）・接続状態・メニューの開閉。
// 偽 daemon と gateway は loopback の空き port。外部ネットワークに出ない。

async function box(page: Page, selector: string) {
  const b = await page.locator(selector).first().boundingBox();
  if (!b) throw new Error(`no box: ${selector}`);
  return b;
}

for (const width of [390, 1440]) {
  test(`page header: パンくず → 見出し・説明 → 主操作（${width}px）`, async ({ page }) => {
    const gateway = await startGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/no-such-page`);
      const main = page.locator("#main");
      const heading = main.getByRole("heading", { level: 1, name: "ページが見つかりません" });
      await expect(heading).toBeVisible();
      await expect(heading).toHaveAttribute("tabindex", "-1");
      await expect(main.locator('[data-screen="*"]')).toBeAttached();

      const crumbs = main.getByRole("navigation", { name: "パンくず" });
      await expect(crumbs).toBeVisible();
      await expect(crumbs.getByRole("link", { name: "ホーム" })).toBeVisible();
      await expect(crumbs.locator('[aria-current="page"]')).toHaveText("ページが見つかりません");
      await expect(main.locator('[data-slot="page-description"]')).toHaveText(
        "この URL の画面はありません。ナビかホームから移ってください。",
      );
      const action = main.locator('[data-slot="page-actions"]').getByRole("link", { name: "ホームへ戻る" });
      await expect(action).toBeVisible();
      const actionBox = await action.boundingBox();
      expect(actionBox?.height ?? 0).toBeGreaterThanOrEqual(44);

      // 見た目の順も DOM の順と同じ。狭い幅では主操作が見出しの下へ折り返す。
      const crumbBox = await box(page, '#main nav[aria-label="パンくず"]');
      const h1Box = await box(page, "#main h1");
      expect(crumbBox.y).toBeLessThan(h1Box.y);
      if (width < 768) expect(actionBox?.y ?? 0).toBeGreaterThan(h1Box.y + h1Box.height - 1);
      else expect(actionBox?.x ?? 0).toBeGreaterThan(h1Box.x + h1Box.width);
      // ページ全体に横スクロールを作らない。
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

      await crumbs.getByRole("link", { name: "ホーム" }).click();
      await expect(page).toHaveURL(`${gateway.base}/`);
      await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeFocused();
    } finally {
      await gateway.close();
    }
  });
}

test("既存の画面は page header の追加 slot を出さない", async ({ page }) => {
  const gateway = await startGateway();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    await expect(page.locator("#main").getByRole("navigation", { name: "パンくず" })).toHaveCount(0);
    await expect(page.locator('#main [data-slot="page-actions"]')).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});

test("接続状態: 接続済み → 切断 → 復帰を語と tone で見せる（スマホ幅でも隠さない）", async ({ page }) => {
  test.setTimeout(90_000);
  const dir = makeTmpDir("celeris-web-e2e-header-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  let daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const daemonUrl = await daemon.start();
  const port = Number(new URL(daemonUrl).port);
  const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
  try {
    await page.setViewportSize({ width: 360, height: 740 });
    await page.goto(`${gateway.base}/`);
    const status = page.locator("[data-connection]");
    await expect(status).toHaveAttribute("role", "status");
    await expect(status).toBeVisible();
    await expect(status).toHaveAttribute("data-connection", "open", { timeout: 20_000 });
    await expect(status).toHaveText("接続状態: 接続済み");
    await expect(status.locator('[data-slot="badge"]')).toHaveAttribute("data-tone", "success");

    await daemon.close();
    await expect(status).toHaveAttribute("data-connection", "down", { timeout: 20_000 });
    await expect(status).toHaveText("接続状態: 切断");
    await expect(status.locator('[data-slot="badge"]')).toHaveAttribute("data-tone", "danger");
    const banner = page.locator("[data-celeris-down]");
    await expect(banner).toHaveAttribute("role", "alert");
    await expect(banner).toHaveText("celeris に接続できません。復旧すると自動で再取得します。");
    // 停止の帯は danger token の背景と前景。
    const colors = await banner.evaluate((el) => {
      const probe = document.createElement("span");
      probe.className = "bg-danger text-danger-foreground";
      document.body.append(probe);
      const want = getComputedStyle(probe);
      const got = getComputedStyle(el);
      const result = {
        bg: [got.backgroundColor, want.backgroundColor],
        fg: [got.color, want.color],
      };
      probe.remove();
      return result;
    });
    expect(colors.bg[0]).toBe(colors.bg[1]);
    expect(colors.fg[0]).toBe(colors.fg[1]);

    daemon = createFakeDaemon({ token: FIXTURE_TOKEN, port });
    await daemon.start();
    await expect(banner).toHaveCount(0, { timeout: 20_000 });
    await expect(status).not.toHaveAttribute("data-connection", "down");
  } finally {
    await gateway.close();
    await daemon.close().catch(() => {});
    rmSync(dir, { recursive: true, force: true });
  }
});

test("メニュー: md 未満は 44px のボタンで開閉し、Escape で閉じて focus を戻す", async ({ page }) => {
  const gateway = await startGateway();
  try {
    await page.setViewportSize({ width: 390, height: 800 });
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    const button = page.getByRole("button", { name: "メニュー" });
    const nav = page.getByRole("navigation", { name: "主要", exact: true });
    await expect(button).toHaveAttribute("aria-controls", "shell-nav");
    await expect(button).toHaveAttribute("aria-expanded", "false");
    const b = await button.boundingBox();
    expect(b?.width ?? 0).toBeGreaterThanOrEqual(44);
    expect(b?.height ?? 0).toBeGreaterThanOrEqual(44);
    await expect(nav).toBeHidden();

    await button.click();
    await expect(button).toHaveAttribute("aria-expanded", "true");
    await expect(nav).toBeVisible();
    await expect(nav.getByRole("link", { name: "タスク" })).toHaveAttribute("aria-current", "page");
    await expect(nav.getByRole("list", { name: "日々の仕事" })).toBeVisible();
    await expect(nav.getByRole("list", { name: "組織と管理" })).toBeVisible();
    // 件数が不明の badge は accessible name で読める。
    await expect(nav.getByRole("img", { name: "受信箱の件数は不明" })).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(button).toHaveAttribute("aria-expanded", "false");
    await expect(nav).toBeHidden();
    await expect(button).toBeFocused();

    // 開いて移ると閉じ、移った先の見出しへ focus が移る。
    await button.click();
    await nav.getByRole("link", { name: "案件" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "案件" })).toBeFocused();
    await expect(button).toHaveAttribute("aria-expanded", "false");
    await expect(nav).toBeHidden();
  } finally {
    await gateway.close();
  }
});

test("メニュー: md 以上は幅 --spacing-nav の左列で、メニューのボタンを出さない", async ({ page }) => {
  const gateway = await startGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByRole("heading", { level: 1, name: "タスク" })).toBeVisible();
    await expect(page.getByRole("button", { name: "メニュー" })).toBeHidden();
    const nav = page.getByRole("navigation", { name: "主要", exact: true });
    await expect(nav).toBeVisible();
    await expect(page.locator("[data-connection]")).toBeVisible();
    const column = await box(page, "[data-shell] > header");
    const navWidth = await page.evaluate(() => {
      const value = getComputedStyle(document.documentElement).getPropertyValue("--spacing-nav").trim();
      return Number.parseFloat(value) * Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
    });
    expect(navWidth).toBe(224);
    expect(column.width).toBe(navWidth);
    const main = await box(page, "#main");
    expect(main.x).toBeGreaterThanOrEqual(column.x + column.width);
  } finally {
    await gateway.close();
  }
});
