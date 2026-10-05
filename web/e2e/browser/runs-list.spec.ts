import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";
import { BROWSER_RAW_LIVE_VIEW_URL } from "../support/fake-daemon.mjs";

// /browser の run 一覧（ADR 2026-10-05-browser-department-web-live-view D3.1・D3.3）。本人は T1 の run と待ちの数・
// lease badge を見て run 画面へ移れる。本人でない session は一覧の代わりに本人登録の案内を見る。
// raw の live_view_url は DOM のどこにも出ない。

test("owner sees T1 runs with waits and lease badges and opens the run screen", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser`);
    await expect(page.getByRole("heading", { level: 1, name: "ブラウザ" })).toBeVisible();

    const r1 = page.locator('[data-testid="browser-run-row"][data-task-id="T1"][data-run-id="R1"]');
    await expect(r1).toBeVisible();
    await expect(r1.getByRole("heading", { name: "請求書フォームの入力" })).toBeVisible();
    await expect(r1.getByTestId("browser-run-waits")).toHaveText("待ち 1 件");
    await expect(r1.getByTestId("browser-run-lease")).toHaveText("監視のみ");
    await expect(r1.getByTestId("browser-run-state")).toHaveText("実行中");
    await expect(r1.getByTestId("browser-run-live")).toHaveText("Live View を開けます");

    const r0 = page.locator('[data-testid="browser-run-row"][data-task-id="T1"][data-run-id="R0"]');
    await expect(r0.getByTestId("browser-run-state")).toHaveText("終了");
    await expect(r0.getByTestId("browser-run-live")).toContainText("ブラウザ実行中のみ");
    await expect(r0.getByTestId("browser-run-lease")).toHaveCount(0);

    // 人の対応が要る run（待ちのある T1/R1）が先頭。
    await expect(page.getByTestId("browser-run-row").first()).toHaveAttribute("data-run-id", "R1");
    // 未決の待ち W1 は waits panel に出て、本人なので decision の form が付く。
    await expect(page.getByTestId("browser-waits").getByTestId("browser-decision-form")).toBeVisible();

    expect(await page.content()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    const hrefs = await page
      .locator("a[href], iframe[src]")
      .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("href") ?? n.getAttribute("src") ?? ""));
    for (const href of hrefs) expect(href).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);

    await r1.getByTestId("browser-run-open").click();
    await expect(page).toHaveURL(`${gateway.base}/browser/runs/T1/R1`);
    expect(await page.content()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);

    await page.goBack();
    await r1.getByRole("link", { name: /task 詳細/ }).click();
    await expect(page).toHaveURL(`${gateway.base}/tasks/T1`);
  } finally {
    await gateway.close();
  }
});

test("360px stacks runs as cards and wraps ids without horizontal overflow", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser`);
    const r1 = page.locator('[data-testid="browser-run-row"][data-run-id="R1"]');
    await expect(r1.getByTestId("browser-run-lease")).toHaveText("監視のみ");
    const box = await r1.boundingBox();
    expect(box?.width ?? 0).toBeLessThanOrEqual(360);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    const open = await r1.getByTestId("browser-run-open").boundingBox();
    expect(open?.height ?? 0).toBeGreaterThanOrEqual(44);
  } finally {
    await gateway.close();
  }
});

test("a non-owner session sees the owner-session notice instead of the list", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOther(page);
    await page.goto(`${gateway.base}/browser`);
    await expect(page.getByTestId("browser-owner-required")).toBeVisible();
    await expect(page.getByTestId("browser-run-row")).toHaveCount(0);
    expect(await page.content()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
  } finally {
    await gateway.close();
  }
});
