import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

test("parity: /sources /harnesses 一覧・詳細を読み取り専用で表示する", async ({ page }) => {
  const dir = makeTmpDir("celeris-sources-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  try {
    await page.goto(`${gateway.base}/sources`);
    const list = page.getByRole("region", { name: "source 一覧（2）" });
    await expect(list.getByRole("link", { name: "claude-oauth" })).toBeVisible();
    await expect(list).toContainText("到達可能");
    await page.getByRole("link", { name: "claude-oauth" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "claude-oauth" })).toBeVisible();
    await expect(page.getByRole("region", { name: "accounts（1）" })).toContainText("main");
    await expect(page.getByRole("region", { name: /models/ })).toContainText("claude-opus");
    await page.goto(`${gateway.base}/harnesses`);
    const harness = page.getByRole("region", { name: "claude-code" });
    await expect(harness).toContainText("anthropic");
    await expect(harness).toContainText("必要能力: repo");
    await expect(harness.getByRole("button")).toHaveCount(0);
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
