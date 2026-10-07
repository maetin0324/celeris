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
    await expect(page.getByRole("row", { name: /claude-opus-4 Claude Opus 4/ })).toContainText("割り当て frontier");
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

  test("上書きの drawer で別名を設定でき、消すと戻る", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    await page.getByRole("button", { name: "上書きを編集 kimi-k2" }).click();
    const drawer = page.getByRole("dialog", { name: /上書き: kimi-k2/ });
    await expect(drawer.getByLabel("tier を固定")).toHaveCount(0);
    await drawer.getByLabel("別名（alias）").fill("kimi");
    await drawer.getByRole("button", { name: "上書きを保存" }).click();
    await expect(drawer).toBeHidden();
    const row = page.locator('[data-source="opencode-go"]').getByRole("row", { name: /kimi-k2/ });
    await expect(row).toContainText("別名: kimi");
    await page.getByRole("button", { name: "上書きを編集 kimi-k2" }).click();
    await page
      .getByRole("dialog", { name: /上書き: kimi-k2/ })
      .getByRole("button", { name: "上書きを消す" })
      .click();
    await expect(page.locator('[data-source="opencode-go"]').getByRole("row", { name: /kimi-k2/ })).not.toContainText(
      "別名",
    );
  });

  test("モデルごとに複数役割を設定し、同じ役割の複数モデルを並べ替える", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    const toggle = (model: string, tier: string) =>
      page.getByRole("checkbox", { name: `opencode-go ${model} ${tier}`, exact: true });
    await toggle("glm-5", "standard").check();
    await toggle("glm-5", "frontier").check();
    await toggle("kimi-k2", "standard").check();
    await expect(page.getByRole("button", { name: "変更を保存" })).toHaveCount(0);
    await page.getByRole("button", { name: "変更の影響を確認" }).click();
    await expect(page.getByLabel("役割の変更の影響")).toContainText("glm-5");
    await page.getByRole("button", { name: "変更を保存" }).click();
    await expect(page.getByText("未保存:")).toHaveCount(0);
    await page.reload();
    await expect(toggle("glm-5", "standard")).toBeChecked();
    await expect(toggle("glm-5", "frontier")).toBeChecked();
    await expect(toggle("kimi-k2", "standard")).toBeChecked();
    await page.getByLabel("役割ごとの表示").selectOption("standard");
    await page.getByRole("button", { name: "kimi-k2 を上へ" }).click();
    await page.getByRole("button", { name: "変更の影響を確認" }).click();
    await page.getByRole("button", { name: "変更を保存" }).click();
    await expect(page.getByText("未保存:")).toHaveCount(0);
    await page.reload();
    await page.getByLabel("役割ごとの表示").selectOption("standard");
    const rows = page.getByRole("region", { name: "standard の候補と順位" }).getByRole("row");
    await expect(rows.nth(2)).toContainText("kimi-k2");
    await expect(rows.nth(3)).toContainText("glm-5");
    await page.getByLabel("役割ごとの表示").selectOption("");
    await page.getByLabel("供給元で絞り込み").selectOption("opencode-go");
    await page.getByLabel("モデルを検索").fill("glm");
    await expect(page.getByRole("region", { name: "モデルごとの役割" }).getByRole("row")).toHaveCount(2);
    await page.getByLabel("モデルを検索").fill("");
    await page.getByLabel("状態で絞り込み").selectOption("消失");
    await expect(page.getByRole("region", { name: "モデルごとの役割" })).toContainText("retired-model");
  });

  test("役割から全モデルを外すと reload 後も config に戻らない", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    const table = page.getByRole("region", { name: "モデルごとの役割" });
    const toggles = table.getByRole("checkbox", { name: / standard$/ });
    await expect(toggles.first()).toBeVisible();
    for (const toggle of await toggles.all()) if (await toggle.isChecked()) await toggle.uncheck();
    await page.getByRole("button", { name: "変更の影響を確認" }).click();
    await expect(page.getByLabel("役割の変更の影響")).toContainText("候補なし");
    await page.getByRole("button", { name: "変更を保存" }).click();
    await expect(page.getByText("未保存:")).toHaveCount(0);
    await page.reload();
    await expect(
      page.getByRole("checkbox", { name: "claude-oauth claude-sonnet-4 standard", exact: true }),
    ).not.toBeChecked();
    await page.getByLabel("役割ごとの表示").selectOption("standard");
    await expect(page.getByRole("region", { name: "standard の候補と順位" }).getByRole("row")).toHaveCount(1);
  });

  test("opencode go を使う: provider を追加するとボタンが消える", async ({ page }) => {
    await page.goto(`${gateway.base}/models`);
    await page.getByRole("button", { name: "opencode go を使う（provider を追加）" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "provider opencode-go を追加" }).click();
    await expect(page.getByRole("button", { name: "opencode go を使う（provider を追加）" })).toHaveCount(0);
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
