import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startBrowserGateway } from "../support/browser-gateway";

const path = "/browser/settings";
const screenshotDir = process.env.BROWSER_SETTINGS_SCREENSHOT_DIR;
test.use({ bypassCSP: true });

async function captureWidths(page: import("@playwright/test").Page, stage: string) {
  if (!screenshotDir) return;
  mkdirSync(screenshotDir, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth), `${stage} @${width}`).toBeLessThanOrEqual(
      width,
    );
    await page.screenshot({ path: join(screenshotDir, `settings-${stage}-${width}.png`), fullPage: true });
  }
}

test("loopback の初期値を表示し、確認して設定を保存する", async ({ page }) => {
  const gateway = await startBrowserGateway();
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}${path}`);
    await expect(page.getByRole("heading", { level: 1, name: "ブラウザ実行課の設定" })).toBeVisible();
    const origins = page.getByRole("list", { name: "許可 origin の一覧" });
    await expect(origins.getByRole("listitem")).toHaveCount(2);
    await expect(origins).toContainText("http://localhost:3000");
    await expect(origins).toContainText("http://127.0.0.1:3000");
    await page.setViewportSize({ width: 360, height: 800 });
    expect(await seriousViolations(page)).toEqual([]);
    for (const button of await page.getByRole("button").all()) {
      if (!(await button.isVisible())) continue;
      const box = await button.boundingBox();
      expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
      expect(box?.width ?? 0).toBeGreaterThanOrEqual(44);
    }
    await captureWidths(page, "before");
    await page.getByLabel("追加する origin").fill("https://billing.example.com");
    await page.getByRole("button", { name: "追加", exact: true }).click();
    await page.getByRole("button", { name: "http://127.0.0.1:3000 を削除" }).click();
    await page.getByLabel("使える harness（カンマ区切り）").fill("codex, claude-code");
    await page.getByLabel("既定の harness").fill("codex");
    await page.getByLabel("最大試行回数").fill("3");
    await page.getByLabel("使える credential policy ID（1 行に 1 件）").fill("billing-policy");
    await page.getByLabel("policy と identity の対応（policy ID=identity ID）").fill("billing-policy=ID1");
    await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("許可 origin の縮小は既存 task の次の run");
    expect(gateway.daemon.browserSettingsEvents).toHaveLength(0);
    await dialog.getByRole("button", { name: "設定を保存" }).click();
    await expect(page.getByRole("status").filter({ hasText: "設定を保存しました" })).toBeVisible();
    await expect(origins).toContainText("https://billing.example.com");
    await expect(origins).not.toContainText("127.0.0.1");
    await expect(page.getByText("変更者: admin")).toBeVisible();
    await captureWidths(page, "after");
    const event = gateway.daemon.browserSettingsEvents[0];
    expect(event?.actor).toBe("admin");
    expect(event?.ts).toBe("2026-10-06T01:23:45Z");
    expect(event?.after).toMatchObject({
      browser: {
        allowed_domains: ["http://localhost:3000", "https://billing.example.com"],
        credential_policy_ids: ["billing-policy"],
        credential_identity_ids: { "billing-policy": "ID1" },
      },
      harnesses: { allowed: ["codex", "claude-code"], default: "codex" },
      budget: { max_attempts: 3, max_lane: "standard" },
    });
  } finally {
    await gateway.close();
  }
});

test("不正 scheme・全体 wildcard・public suffix wildcard は項目別に拒否する", async ({ page }) => {
  const gateway = await startBrowserGateway();
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}${path}`);
    for (const value of ["ftp://example.com", "http://example.com", "*", "https://*.com", "https://*.co.uk"]) {
      await page.getByLabel("追加する origin").fill(value);
      await page.getByRole("button", { name: "追加", exact: true }).click();
      await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "設定を保存" }).click();
      await expect(page.getByText("expected valid browser origins (scheme or wildcard)")).toBeVisible();
      expect(gateway.daemon.browserSettingsEvents).toHaveLength(0);
      await page.getByRole("alertdialog").getByRole("button", { name: "戻る" }).click();
      await page.getByRole("button", { name: `${value} を削除` }).click();
    }
  } finally {
    await gateway.close();
  }
});
