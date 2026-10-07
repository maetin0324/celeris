import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, noticesFixture } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// beforeAll の一時 dir・server をこの file の試験で共有するので、file 内は 1 worker で順に流す（2026-10-04 の並列化と同じ扱い）。
test.describe.configure({ mode: "default" });

// /notifications（ADR-0133 D5）: 束の一覧・未読と種類の絞り込み（URL）・1 件の既読・確認付きの一括既読・頁送り。
const dir = makeTmpDir("celeris-notifications-");
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
let gateway: Awaited<ReturnType<typeof startGateway>>;
test.beforeAll(async () => {
  gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
});
test.beforeEach(() => {
  daemon.setNotices(noticesFixture());
});
test.afterAll(async () => {
  await gateway.close();
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

const unreadIds = () => daemon.inbox.notices.filter((notice) => !notice.read_at).map((notice) => notice.id);

test("parity: 通知 束の一覧と未読・種類の絞り込み", async ({ page }) => {
  await page.goto(`${gateway.base}/notifications`);
  await expect(page.getByRole("heading", { level: 1, name: "通知" })).toBeVisible();
  const list = page.getByRole("list", { name: "通知の一覧" });
  await expect(list.getByRole("listitem")).toHaveCount(4);
  const bundle = list.getByRole("listitem", { name: "3 件の task が完了" });
  await expect(bundle.getByText("3 件の束")).toBeVisible();
  await expect(bundle.getByText("未読")).toBeVisible();
  await expect(bundle.getByRole("link", { name: "タスク" })).toHaveAttribute("href", "/tasks/T1");
  await expect(
    list.getByRole("listitem", { name: "nightly の検査が失敗" }).getByRole("link", { name: "task T3" }),
  ).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "未読 3 件" })).toBeVisible();

  await page.getByLabel("表示").selectOption("unread");
  await expect(page).toHaveURL(/unread=true/);
  await expect(list.getByRole("listitem")).toHaveCount(3);
  await page.getByLabel("種類").selectOption("bad_news");
  await expect(page).toHaveURL(/kind=bad_news/);
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await expect(list.getByRole("listitem", { name: "nightly の検査が失敗" })).toBeVisible();

  // URL から開き直しても同じ絞り込み。
  await page.goto(`${gateway.base}/notifications?kind=report`);
  await expect(page.getByLabel("種類")).toHaveValue("report");
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await expect(list.getByRole("link", { name: "報告の一覧" })).toHaveAttribute("href", "/reports");
});

test("parity: 通知 1 件の既読と nav の未読数", async ({ page }) => {
  await page.goto(`${gateway.base}/notifications`);
  const nav = page.getByRole("navigation", { name: "主要", exact: true });
  await expect(nav.getByRole("img", { name: "未読の通知 3 件" })).toBeVisible();
  const row = page.getByRole("listitem", { name: "日次の報告" });
  await row.getByRole("button", { name: "「日次の報告」を既読にする" }).click();
  await expect(row.getByRole("button", { name: /既読にする/ })).toHaveCount(0);
  await expect(row.getByText("既読", { exact: true })).toBeVisible();
  expect(unreadIds()).toEqual(["N1", "N3"]);
  await expect(nav.getByRole("img", { name: "未読の通知 2 件" })).toBeVisible();
});

test("parity: 通知 一括既読は確認してから送る", async ({ page }) => {
  await page.goto(`${gateway.base}/notifications?kind=task_done`);
  await expect(page.getByRole("list", { name: "通知の一覧" }).getByRole("listitem")).toHaveCount(1);
  await page.getByRole("button", { name: "すべて既読にする" }).click();
  const dialog = page.getByRole("alertdialog", { name: "通知をまとめて既読にする" });
  await expect(dialog.getByText("種類「task の完了」の未読の通知")).toBeVisible();
  // 取り消しでは送らない。
  await dialog.getByRole("button", { name: "戻る" }).click();
  await expect(dialog).toHaveCount(0);
  expect(daemon.requests.some((request) => request.path === "/api/v1/notifications/read-all")).toBe(false);
  // 絞り込み中は、その種類だけを既読にする。
  await page.getByRole("button", { name: "すべて既読にする" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "既読にする" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(unreadIds()).toEqual(["N2", "N3"]);
  await page.getByLabel("種類").selectOption("");
  await expect(page.getByRole("status").filter({ hasText: "未読 2 件" })).toBeVisible();
  await page.getByRole("button", { name: "すべて既読にする" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "既読にする" }).click();
  await expect(page.getByRole("status").filter({ hasText: "未読はありません" })).toBeVisible();
  expect(unreadIds()).toEqual([]);
  await expect(page.getByRole("button", { name: "すべて既読にする" })).toBeDisabled();
});

test("parity: 通知 next_before の頁送り", async ({ page }) => {
  const many = Array.from({ length: 60 }, (_, i) => ({
    ...noticesFixture()[1],
    id: `M${i}`,
    group_key: `report:M${i}`,
    title: `報告 ${String(i).padStart(2, "0")}`,
    last_at: new Date(Date.UTC(2026, 8, 1, 0, 60 - i)).toISOString(),
  }));
  daemon.setNotices(many);
  await page.goto(`${gateway.base}/notifications`);
  const list = page.getByRole("list", { name: "通知の一覧" });
  await expect(list.getByRole("listitem")).toHaveCount(50);
  await expect(list.getByRole("listitem", { name: "報告 00" })).toBeVisible();
  await page.getByRole("button", { name: "古い通知を見る" }).click();
  await expect(page).toHaveURL(/before=/);
  await expect(list.getByRole("listitem")).toHaveCount(10);
  await expect(list.getByRole("listitem", { name: "報告 59" })).toBeVisible();
  await expect(page.getByRole("button", { name: "古い通知を見る" })).toHaveCount(0);
  await page.getByRole("button", { name: "最新に戻る" }).click();
  await expect(page).not.toHaveURL(/before=/);
  await expect(list.getByRole("listitem")).toHaveCount(50);
});

test("parity: 通知 360px で横に溢れない", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${gateway.base}/notifications`);
  await expect(page.getByRole("list", { name: "通知の一覧" }).getByRole("listitem")).toHaveCount(4);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});

test("parity: /console の受信箱・通知の入口と件数", async ({ page }) => {
  // 詳細な入口と件数は監視用 Console に移した。既存の導線と件数を検証する。
  await page.goto(`${gateway.base}/console`);
  const entries = page.getByRole("navigation", { name: "受信箱と通知" });
  // 件数は shell の常駐 query の cache を読むだけなので、取得が済むと出る。
  await expect(entries.getByRole("link", { name: /^通知/ })).toContainText("未読 3 件");
  await expect(entries.getByRole("link", { name: /^受信箱/ })).toContainText(/判断待ち \d+ 件/);
  await entries.getByRole("link", { name: /^通知/ }).click();
  await expect(page).toHaveURL(/\/notifications$/);
  await expect(page.getByRole("heading", { level: 1, name: "通知" })).toBeVisible();
});
