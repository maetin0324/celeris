import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

test.describe.configure({ mode: "default" });

// 設定画面・routing 画面が gateway 経由で daemon の /api/v1/config/*・/api/v1/routing/* を一度だけ前置して読み、
// 値を表示すること（2026-10-22: /api/v1 を二重に前置して 404 になり、何も表示しなかった）。
test.describe("settings / routing through the gateway", () => {
  const dir = makeTmpDir("celeris-settings-routing-");
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
  const doubled = () => daemon.requests.filter((r) => r.path.startsWith("/api/v1/v1/"));

  test("設定画面: 欄の値と変更の履歴が出る", async ({ page }) => {
    await page.goto(`${gateway.base}/settings`);
    await expect(page.getByText("/home/u/.config/celeris/config.toml")).toBeVisible();
    await expect(page.locator('[id="config-execution.max_replans"]')).toHaveValue("3");
    await expect(page.getByText("変更の履歴（1）")).toBeVisible();
    for (const route of ["/api/v1/config/fields", "/api/v1/config/events"])
      expect(daemon.requests.some((r) => r.method === "GET" && r.path === route)).toBe(true);
    expect(doubled()).toEqual([]);
  });

  test("routing 画面: 段階と根拠表が出る", async ({ page }) => {
    await page.goto(`${gateway.base}/routing`);
    await expect(page.getByText("段階（1）")).toBeVisible();
    await expect(page.getByText("claude_oauth / claude-opus")).toBeVisible();
    await expect(page.getByText("評価を開始")).toBeVisible();
    for (const route of ["/api/v1/routing/stages", "/api/v1/routing/evidence"])
      expect(daemon.requests.some((r) => r.method === "GET" && r.path === route)).toBe(true);
    expect(doubled()).toEqual([]);
  });
});
