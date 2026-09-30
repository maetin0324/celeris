import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-knowledge-"));
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

test("parity: /knowledge 検索・閲覧・保存", async ({ page }) => {
  await page.goto(`${gateway.base}/knowledge`);
  await page.getByLabel("検索").fill("Demo");
  await page.getByRole("button", { name: "検索" }).click();
  await expect(page).toHaveURL(/q=Demo/);
  await page.getByRole("link", { name: "Demo knowledge" }).click();
  await expect(page).toHaveURL(/path=projects%2Fdemo.md/);
  await expect(page.getByText("Knowledge body")).toBeVisible();
  await page.getByRole("link", { name: "編集" }).click();
  await page.getByLabel("本文").fill("# Changed");
  await page.route("**/api/knowledge/page", async (route) => {
    if (route.request().method() === "PUT")
      await route.fulfill({
        status: 422,
        contentType: "application/json",
        body: JSON.stringify({ detail: "本文が不正です" }),
      });
    else await route.continue();
  });
  await page.getByRole("button", { name: "保存" }).click();
  await expect(page.getByRole("alert")).toHaveText("本文が不正です");
  await expect(page.getByLabel("本文")).toHaveValue("# Changed");
});

test("parity: /knowledge/inbox 採用・却下", async ({ page }) => {
  await page.goto(`${gateway.base}/knowledge/inbox`);
  await expect(page.getByRole("heading", { name: "New knowledge", level: 2 })).toBeVisible();
  await page.getByRole("button", { name: "採用" }).click();
  await expect(page.getByText("操作が完了しました")).toBeVisible();
  await page.getByRole("button", { name: "却下" }).click();
  await expect.poll(() => daemon.requests.filter((request) => request.path.endsWith("/reject")).length).toBe(1);
  expect(daemon.requests.find((request) => request.path.endsWith("/accept"))?.body).toContain("projects/new.md");
});
