import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

test("ホームは直近の人の会話を URL に選び、再読込後も本文と composer を表示する", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.goto(`${gateway.base}/`);
    await expect(page).toHaveURL(/\?thread=chat-main$/);
    await expect(page.getByText("進捗を教えて")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "CoS へのメッセージ" })).toBeVisible();
    await page.reload();
    await expect(page.getByText("進捗を教えて")).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Console", exact: true }).first()).toHaveAttribute("href", "/console");
  } finally {
    await gateway.close();
  }
});

test("共有した thread URL で別の会話を開く", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.goto(`${gateway.base}/?thread=chat-history`);
    await expect(page.getByText("履歴 240", { exact: true })).toBeVisible();
    await expect(page).toHaveURL(/\?thread=chat-history$/);
  } finally {
    await gateway.close();
  }
});

test("320px でも composer は下部タブバーの上にあり横へ溢れない", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 320, height: 720 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    const composer = page.getByRole("region", { name: "メッセージ入力" });
    const tabs = page.getByRole("navigation", { name: "主要（モバイル）" });
    await expect(composer).toBeVisible();
    const composerBox = await composer.boundingBox();
    const tabBox = await tabs.boundingBox();
    expect(composerBox && tabBox && composerBox.y + composerBox.height <= tabBox.y).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: "ファイルを添付" }).click();
    await (await chooser).setFiles({ name: "note.txt", mimeType: "text/plain", buffer: Buffer.from("memo") });
    await expect(page.getByRole("list", { name: "添付ファイル" })).toContainText("note.txt");
  } finally {
    await gateway.close();
  }
});

test("人の会話が無ければ新しい thread を作る", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    for (const id of ["chat-main", "chat-history", "chat-legacy"]) {
      const response = await fetch(`${gateway.base}/api/chat/threads/${id}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ status: "archived", expected_revision: 1 }),
      });
      expect(response.status).toBe(200);
    }
    await page.goto(`${gateway.base}/`);
    await expect(page).toHaveURL(/\?thread=chat-new-1$/);
    await expect(page.getByRole("textbox", { name: "CoS へのメッセージ" })).toBeVisible();
  } finally {
    await gateway.close();
  }
});
