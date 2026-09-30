import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-reports-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const report = {
  id: "R1",
  headline: "実行結果",
  kind: "result",
  level: 0,
  node_id: "cos",
  created_at: "2026-09-30T00:00:00Z",
  body: "# 結果",
  read_at: null,
  sources: ["R0"],
};
const source = { ...report, id: "R0", headline: "元の報告", body: "出典の本文", sources: [] };
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: {
    ...defaultFixtures,
    "/api/v1/reports": { items: [report] },
    "/api/v1/reports/read": { updated: 1 },
    "/api/v1/notify/test": { sent: true },
    "/api/v1/reports/R1": { report, sources_expanded: [source] },
    "/api/v1/daemon": {
      ...(defaultFixtures["/api/v1/daemon"] as object),
      reports: { notify_now: false, unread_bad_news: 0, unread_secretary: 1 },
    },
  },
});
let gateway: Awaited<ReturnType<typeof startGateway>>;
test.beforeAll(async () => {
  gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
});
test.afterAll(async () => {
  await gateway.close();
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

test("parity: /reports 絞り込み・既読・通知試験・展開", async ({ page }) => {
  await page.goto(`${gateway.base}/reports`);
  await expect(page.getByText("実行結果")).toBeVisible();
  await page.getByLabel("表示").selectOption("all");
  await page.getByLabel("段").selectOption("1");
  await expect(page).toHaveURL(/filter=all/);
  await expect(page).toHaveURL(/level=1/);
  await page.getByRole("button", { name: "展開" }).click();
  await expect(page.getByText("元の報告").last()).toBeVisible();
  expect(daemon.requests.some((request) => request.path === "/api/v1/reports/R1")).toBe(true);
  await page.getByRole("button", { name: "既読にする", exact: true }).click();
  await expect(page.getByText("操作が完了しました").first()).toBeVisible();
  await expect(page.getByText("元の報告").last()).toBeVisible();
  await page.getByRole("button", { name: "通知を試す" }).click();
  await expect
    .poll(() => daemon.requests.some((request) => request.path === "/api/v1/notify/test" && request.method === "POST"))
    .toBe(true);
});

test("parity: /reports/:id 展開の取得", async ({ page }) => {
  await page.goto(`${gateway.base}/reports`);
  await page.getByRole("button", { name: "展開" }).click();
  await expect(page.getByText("出典の本文")).toBeVisible();
});

test("/reports fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  mkdirSync(out as string, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${gateway.base}/reports`);
    await expect(page.getByText("実行結果")).toBeVisible();
    await page.getByRole("button", { name: "展開" }).click();
    await page.screenshot({ path: path.join(out as string, `reports-fixture-${width}.png`), fullPage: true });
  }
});
