import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

const dir = makeTmpDir("celeris-reports-");
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

// 報告は本文を読むだけの互換の画面（web ADR 2026-10-04 D4）。既読・通知試験は通知（/notifications）へ寄せた。
test("parity: /reports 絞り込み・既読・通知試験・展開", async ({ page }) => {
  await page.goto(`${gateway.base}/reports`);
  await expect(page.getByRole("heading", { level: 1, name: "報告" })).toBeVisible();
  await expect(page.getByText("実行結果")).toBeVisible();
  await page.getByLabel("段").selectOption("1");
  await expect(page).toHaveURL(/level=1/);
  await page.getByRole("button", { name: "展開" }).click();
  await expect(page.getByText("元の報告").last()).toBeVisible();
  expect(daemon.requests.some((request) => request.path === "/api/v1/reports/R1")).toBe(true);
  // 報告の画面では既読にしない（旧 POST /reports/read を呼ぶ操作が無い）。
  await expect(page.getByRole("button", { name: /既読にする/ })).toHaveCount(0);
  await page.getByRole("link", { name: "通知で報告の知らせを見る" }).click();
  await expect(page).toHaveURL(/\/notifications\?kind=report/);
  await expect(page.getByRole("heading", { level: 1, name: "通知" })).toBeVisible();
  await page.getByRole("button", { name: "「日次の報告」を既読にする" }).click();
  await expect(page.getByRole("button", { name: "「日次の報告」を既読にする" })).toHaveCount(0);
  await page.getByRole("button", { name: "通知を試す" }).click();
  await expect
    .poll(() => daemon.requests.some((request) => request.path === "/api/v1/notify/test" && request.method === "POST"))
    .toBe(true);
  expect(daemon.requests.some((request) => request.path === "/api/v1/reports/read")).toBe(false);
});

test("parity: /reports/:id 展開の取得", async ({ page }) => {
  await page.goto(`${gateway.base}/reports`);
  await page.getByRole("button", { name: "展開" }).click();
  await expect(page.getByText("出典の本文")).toBeVisible();
  // 通知の行き先（/reports?report=<id>）は、その報告を開いた状態で出す。
  await page.goto(`${gateway.base}/reports?report=R1`);
  await expect(page.getByRole("button", { name: "閉じる" })).toBeVisible();
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

// ADR 2026-11-11-cos-fallback-timing D6: 知らせの未読が増えてもブラウザ通知は出さない（人への知らせは Discord だけで、
// CoS の判定の後に届く）。未読の badge は出る。
test("parity-x: 未読が増えてもブラウザ通知を出さない", async ({ page, context }) => {
  const unread = { unread: 2, events: 3, by_kind: { bad_news: 1, report: 1 } };
  await context.addInitScript(() => {
    (window as Window & { __notifications?: string[] }).__notifications = [];
    Object.defineProperty(window, "Notification", {
      configurable: true,
      value: class {
        static permission = "granted";
        constructor(title: string, options?: { body?: string }) {
          (window as Window & { __notifications?: string[] }).__notifications?.push(`${title}: ${options?.body ?? ""}`);
        }
      },
    });
  });
  await page.route("**/api/notifications/unread-count", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(unread) }),
  );
  await page.goto(`${gateway.base}/reports`);
  await expect(page.getByLabel("未読の通知 2 件")).toBeVisible();
  unread.events = 5;
  unread.unread = 3;
  daemon.sendEvent("notifications_changed", {});
  await expect(page.getByLabel("未読の通知 3 件")).toBeVisible();
  expect(await page.evaluate(() => (window as Window & { __notifications?: string[] }).__notifications?.length)).toBe(
    0,
  );
  expect(await page.evaluate(() => Object.keys(localStorage).some((key) => key.includes("notifications-seen")))).toBe(
    false,
  );
});
