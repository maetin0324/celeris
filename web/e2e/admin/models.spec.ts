import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// 偽 daemon の状態（発見・上書き）をこの file の試験で共有するので、file 内は 1 worker で順に流す。
test.describe.configure({ mode: "default" });

// /models（モデル一覧）と /accounts の使用量 3 本。
test.describe("models", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-models-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  let gateway: Awaited<ReturnType<typeof startGateway>>;
  test.beforeAll(async () => {
    gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  });
  test.afterAll(async () => {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  });

  test("供給元ごとの節を出し、消失したモデルを語で示す", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    await expect(page.getByRole("heading", { level: 1, name: "モデル" })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: /OpenCode Go（subscription）/ })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: /Claude（subscription）/ })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: /self-host qwen/ })).toBeVisible();
    const gone = page.locator('[data-model-state="消失"]');
    await expect(gone).toHaveCount(1);
    await expect(gone).toContainText("retired-model");
    await expect(page.getByRole("row", { name: /qwen3-coder/ })).toContainText("固定 cheap");
  });

  test("発見を実行すると最終発見と最終確認が更新される", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    const section = page.locator('[data-source="opencode-go"]');
    const before = await section.getByRole("row", { name: /glm-5/ }).innerText();
    await expect(section).toContainText("成功 / 3 件");
    await page.waitForTimeout(1100);
    await section.getByRole("button", { name: "発見を実行" }).click();
    await expect(section.getByRole("status")).toContainText("操作が完了しました");
    await expect(section).toContainText("成功 / 2 件");
    await expect(section.getByRole("row", { name: /glm-5/ })).not.toHaveText(before);
  });

  test("上書きの drawer で tier を固定でき、消すと戻る", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    await page.getByRole("button", { name: "上書きを編集 kimi-k2" }).click();
    const drawer = page.getByRole("dialog", { name: /上書き: kimi-k2/ });
    await drawer.getByLabel("tier を固定").selectOption("frontier");
    await drawer.getByLabel("別名（alias）").fill("kimi");
    await drawer.getByRole("button", { name: "上書きを保存" }).click();
    await expect(drawer).toBeHidden();
    const row = page.getByRole("row", { name: /kimi-k2/ });
    await expect(row).toContainText("固定 frontier");
    await expect(row).toContainText("別名: kimi");
    await page.getByRole("button", { name: "上書きを編集 kimi-k2" }).click();
    await page
      .getByRole("dialog", { name: /上書き: kimi-k2/ })
      .getByRole("button", { name: "上書きを消す" })
      .click();
    await expect(page.getByRole("row", { name: /kimi-k2/ })).not.toContainText("固定");
  });

  test("360px で横に溢れない", async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/models`);
    await expect(page.getByRole("heading", { level: 1, name: "モデル" })).toBeVisible();
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);
  });

  test("accounts: opencode-go は 3 本の meter、claude の 1 か月枠は「不明」", async ({ page }) => {
    await page.goto(`${gateway.base}/accounts`);
    const go = page.getByRole("listitem", { name: "アカウント go-main" });
    await expect(go.getByRole("meter")).toHaveCount(3);
    await expect(go.getByRole("meter", { name: "月間枠（1か月）" })).toHaveAttribute("aria-valuenow", "75");
    const main = page.getByRole("listitem", { name: "アカウント main" });
    await expect(main.getByRole("meter")).toHaveCount(2);
    await expect(main.locator('[data-usage-window="none"]')).toContainText("不明");
  });
});
