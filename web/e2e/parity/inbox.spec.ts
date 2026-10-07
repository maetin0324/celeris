import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import type { InboxItem } from "../../api/generated/types";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, inboxItemsFixture } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

const dir = makeTmpDir("celeris-inbox-");
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
// 受信箱は判断待ち（GET /inbox/items）だけを出し、その場で POST /inbox/items/{id}/answer を返す（ADR-0133）。
// 偽 daemon は状態を持つので、各試験の頭で項目を差し替える。専用操作の項目（cluster_login）を 1 件足す。
const clusterLogin: InboxItem = {
  ...inboxItemsFixture()[0],
  id: "cluster_login:sirius",
  kind: "cluster_login",
  title: "クラスタ sirius に入り直す",
  detail: null,
  options: [],
  recommended: null,
  due_at: null,
  blocking: { tasks: [], units: [], summary: "sirius の 1 task を止めている" },
  blocked_by: [],
  links: [],
  project_id: null,
  task: null,
};
const items = () => [...inboxItemsFixture(), clusterLogin];
const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
let gateway: Awaited<ReturnType<typeof startGateway>>;
test.beforeAll(async () => {
  gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
});
test.beforeEach(() => {
  daemon.setInboxItems(items());
  daemon.inbox.answers.length = 0;
});
test.afterAll(async () => {
  await gateway.close();
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

const row = (page: Page, id: string) => page.locator(`[data-inbox-item="${id}"]`);

test("parity: /inbox 区画表示と承認・409 再取得", async ({ page }) => {
  await page.goto(`${gateway.base}/inbox`);
  await expect(page.getByRole("heading", { level: 1, name: "受信箱" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "判断待ち（6）" })).toBeVisible();
  // 何を決めるか・推奨・期限・止めている範囲が各項目に読める。
  const decision = row(page, "decision:D1");
  await expect(decision.getByRole("heading", { name: "認証方式を決める" })).toBeVisible();
  await expect(decision.getByText("推奨:")).toBeVisible();
  await expect(decision.locator("strong")).toHaveText("gateway の session");
  await expect(decision.locator("time[datetime='2026-10-05T09:00:00Z']")).toBeVisible();
  await expect(decision.getByText("葉 auth を止めている")).toBeVisible();
  await expect(decision.getByRole("link", { name: "web の認証" })).toHaveAttribute("href", "/tasks/T1");
  await expect(row(page, "plan_gate:T2").getByRole("link", { name: "認証方式を決める" })).toHaveAttribute(
    "href",
    "#item-decision%3AD1",
  );
  // 推奨の選択肢で答えると項目が消える。
  await decision.getByLabel(/理由・note/).fill("cookie だけにする");
  await decision.getByRole("button", { name: "gateway の session（推奨）" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "「認証方式を決める」に「gateway の session」と答えました。" }),
  ).toBeVisible();
  await expect(decision).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "判断待ち（5）" })).toBeVisible();
  expect(daemon.inbox.answers).toEqual([{ id: "decision:D1", option: "session", note: "cookie だけにする" }]);
  // 専用操作の項目は答えの欄を出さず、専用画面へ誘導する。
  const cluster = row(page, "cluster_login:sirius");
  await expect(cluster.getByRole("button")).toHaveCount(0);
  await cluster.getByRole("link", { name: "クラスタの画面で操作する" }).click();
  await expect(page).toHaveURL(/\/clusters$/);
  // answer が 409 native_action_required を返したら、専用画面への誘導に切り替える。
  await page.route("**/api/inbox/items/*/answer", (route) =>
    route.fulfill({
      status: 409,
      contentType: "application/problem+json",
      body: JSON.stringify({ code: "native_action_required", detail: "use the linked domain action", status: 409 }),
    }),
  );
  await page.goto(`${gateway.base}/inbox`);
  const failed = row(page, "failed:T3");
  await failed.getByRole("button", { name: "やり直す（推奨）" }).click();
  await expect(failed.getByRole("alert")).toContainText("この項目は専用の画面で操作します。");
  await expect(failed.getByRole("link", { name: "タスク「nightly の検査」で操作する" })).toHaveAttribute(
    "href",
    "/tasks/T3",
  );
});

test("parity-x: 409 再取得・422 表示・二重送信なし", async ({ page }) => {
  let count = 0;
  await page.route("**/api/inbox/items/*/answer", async (route) => {
    count += 1;
    await new Promise((resolve) => setTimeout(resolve, 200));
    if (count === 1) {
      await route.fulfill({
        status: 422,
        contentType: "application/json",
        body: JSON.stringify({ code: "validation", detail: "回答が不正です" }),
      });
    } else {
      await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ code: "conflict" }) });
    }
  });
  await page.goto(`${gateway.base}/inbox`);
  const plan = row(page, "plan_gate:T2");
  // 理由が要る選択は、空のまま送らずに欄の横で止める。
  await plan.getByRole("button", { name: "計画をやり直す" }).click();
  await expect(plan.getByRole("alert")).toHaveText("「計画をやり直す」には理由（note）が要ります。");
  await expect(plan.getByLabel(/理由・note/)).toHaveAttribute("aria-invalid", "true");
  expect(count).toBe(0);
  await plan.getByLabel(/理由・note/).fill("再入力を残す");
  await plan.getByRole("button", { name: "計画をやり直す" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await expect(plan.getByRole("alert")).toHaveText("回答が不正です");
  expect(count).toBe(1);
  await expect(plan.getByLabel(/理由・note/)).toHaveValue("再入力を残す");
  const before = daemon.requests.filter((request) => request.path === "/api/v1/inbox/items").length;
  await plan.getByRole("button", { name: "計画をやり直す" }).click();
  await expect(plan.getByRole("alert")).toContainText("状態が変わりました");
  await expect
    .poll(() => daemon.requests.filter((request) => request.path === "/api/v1/inbox/items").length)
    .toBeGreaterThan(before);
});

test("parity: /inbox 破壊的な選択は確認を経て消える", async ({ page }) => {
  await page.goto(`${gateway.base}/inbox`);
  const plan = row(page, "plan_gate:T2");
  await plan.getByLabel(/理由・note/).fill("範囲が広すぎる");
  await plan.getByRole("button", { name: "取り下げる" }).click();
  const dialog = page.getByRole("alertdialog", { name: "「取り下げる」を選びますか" });
  await expect(dialog).toContainText("task を止める");
  await dialog.getByRole("button", { name: "戻る" }).click();
  await expect(dialog).toHaveCount(0);
  expect(daemon.inbox.answers).toEqual([]);
  await plan.getByRole("button", { name: "取り下げる" }).click();
  await dialog.getByRole("button", { name: "「計画を承認する: 受信箱の画面」を取り下げる" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(plan).toHaveCount(0);
  expect(daemon.inbox.answers).toEqual([{ id: "plan_gate:T2", option: "withdraw", note: "範囲が広すぎる" }]);
});

test("parity: /inbox 案件・種類の絞り込みは URL の search param", async ({ page }) => {
  await page.goto(`${gateway.base}/inbox?project=P1`);
  await expect(page.getByRole("heading", { name: "判断待ち（2）" })).toBeVisible();
  await page.getByLabel("種類").selectOption("decision");
  await expect(page).toHaveURL(/[?&]kind=decision/);
  await expect(page).toHaveURL(/[?&]project=P1/);
  await expect(page.getByRole("heading", { name: "判断待ち（1）" })).toBeVisible();
  await page.getByRole("button", { name: "絞り込みを外す" }).click();
  await expect(page).toHaveURL(/\/inbox$/);
  await expect(page.getByRole("heading", { name: "判断待ち（6）" })).toBeVisible();
});

// S7 の補助: 偽 daemon の実データを表示した状態を 4 幅で保存する。
test("/inbox fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  mkdirSync(out as string, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${gateway.base}/inbox`);
    await expect(page.getByRole("heading", { name: "判断待ち（6）" })).toBeVisible();
    await page.screenshot({ path: path.join(out as string, `inbox-fixture-${width}.png`), fullPage: true });
  }
});

const approvalFixture = {
  id: "A1",
  node_id: "cos",
  question: "**実行**を許可しますか",
  created_at: "2026-09-30T00:00:00Z",
  task_id: "T1",
};
test("parity: /approvals 判定・常設ルール・バッジ", async ({ page }) => {
  // 未決の認可は受信箱で答える。/approvals は件数と誘導・決めた記録・常設ルールだけ（web ADR 2026-10-04 D4）。
  const requests: { path: string; method: string; body?: string }[] = [];
  await page.route("**/api/approvals?**", async (route) => {
    const pending = new URL(route.request().url()).searchParams.get("pending") === "true";
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: pending ? [approvalFixture] : [{ ...approvalFixture, id: "A0", decision: "once" }],
      }),
    });
  });
  await page.route("**/api/standing-rules**", async (route) => {
    requests.push({ path: "rules", method: route.request().method(), body: route.request().postData() ?? undefined });
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [{ id: "S1", rule: "既定の規則", created_at: "2026-09-30T00:00:00Z" }] }),
    });
  });
  await page.goto(`${gateway.base}/approvals`);
  await expect(page.getByRole("heading", { level: 1, name: "承認" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "認可待ち" })).toBeVisible();
  await expect(page.getByText("受信箱に未決の認可が 1 件あります。")).toBeVisible();
  // 決める操作はこの画面に置かない。
  await expect(page.getByRole("button", { name: "今回だけ認める" })).toHaveCount(0);
  const history = page.getByRole("list", { name: "決めた認可" });
  await expect(history.getByText("今回だけ認めた")).toBeVisible();
  await page.getByLabel("規則文").fill("新しい規則");
  await page.getByRole("button", { name: "追加" }).click();
  await expect
    .poll(() => requests.filter((request) => request.path === "rules" && request.method === "POST").length)
    .toBe(1);
  // 常設ルールの削除は確認を経る。
  await page.getByRole("button", { name: "削除" }).click();
  const dialog = page.getByRole("alertdialog", { name: "常設ルールを削除しますか" });
  await dialog.getByRole("button", { name: "常設ルールを削除" }).click();
  await expect
    .poll(() => requests.filter((request) => request.path === "rules" && request.method === "DELETE").length)
    .toBe(1);
  // 受信箱の認可の絞り込みへ移って、その場で答えられる。
  await page.getByRole("link", { name: "受信箱で認可を判断する" }).click();
  await expect(page).toHaveURL(/\/inbox\?kind=authorization$/);
  await expect(page.getByRole("heading", { name: "判断待ち（1）" })).toBeVisible();
  await expect(row(page, "authorization:A1").getByRole("button", { name: "認可する（推奨）" })).toBeVisible();
});

test("/approvals fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  mkdirSync(out as string, { recursive: true });
  await page.route("**/api/approvals?**", async (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [approvalFixture] }) }),
  );
  await page.route("**/api/standing-rules", async (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [] }) }),
  );
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${gateway.base}/approvals`);
    await expect(page.getByRole("heading", { name: "認可待ち" })).toBeVisible();
    await page.screenshot({ path: path.join(out as string, `approvals-fixture-${width}.png`), fullPage: true });
  }
});
