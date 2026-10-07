import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

const dir = makeTmpDir("celeris-knowledge-");
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
  // 出典・scope・更新日を読める形で出す。
  await expect(page.getByText("出典", { exact: true })).toBeVisible();
  await expect(page.getByText("scope", { exact: true })).toBeVisible();
  await expect(page.getByText("更新日", { exact: true })).toBeVisible();
  // 採否は確認 dialog で正本に入る内容を見てから確定する。
  await page.getByRole("button", { name: "採用" }).click();
  const accept = page.getByRole("alertdialog");
  await expect(accept).toContainText("取り込み先 projects/new.md");
  await expect(accept).toContainText("scope 未設定");
  expect(daemon.requests.filter((request) => request.path.endsWith("/accept"))).toHaveLength(0);
  await accept.getByRole("button", { name: "「New knowledge」を正本に採用" }).click();
  await expect(accept).toBeHidden();
  await expect(page.getByText("操作が完了しました")).toBeVisible();
  await page.getByRole("button", { name: "却下" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "「New knowledge」を却下" }).click();
  await expect.poll(() => daemon.requests.filter((request) => request.path.endsWith("/reject")).length).toBe(1);
  expect(daemon.requests.find((request) => request.path.endsWith("/accept"))?.body).toContain("projects/new.md");
});

test("parity: /knowledge/skills 一覧・create・name・edit・削除", async ({ page }) => {
  await page.goto(`${gateway.base}/knowledge/skills`);
  await page.getByRole("link", { name: "demo" }).click();
  await expect(page).toHaveURL(/name=demo/);
  await expect(page.getByText("Description")).toBeVisible();
  await page.getByRole("link", { name: "編集" }).click();
  await expect(page).toHaveURL(/edit=1/);
  await page.getByLabel("SKILL.md").fill("# Updated");
  await page.getByLabel("ファイルのパス").fill("references/new.md");
  await page.getByLabel("ファイルの内容").fill("content");
  await page.getByRole("button", { name: "保存" }).click();
  await expect
    .poll(
      () =>
        daemon.requests.filter((request) => request.method === "PUT" && request.path.endsWith("/skills/demo")).length,
    )
    .toBe(1);
  const sent = daemon.requests.find((request) => request.method === "PUT" && request.path.endsWith("/skills/demo"));
  expect(JSON.parse(sent?.body ?? "null")).toEqual({
    skill_md: "# Updated",
    files: [{ path: "references/new.md", content: "content" }],
  });
  await page.getByRole("link", { name: "作成" }).click();
  await expect(page).toHaveURL(/create=1/);
  await page.getByLabel("名前").fill("new-skill");
  await page.route("**/api/skills/new-skill", async (route) =>
    route.fulfill({
      status: 422,
      contentType: "application/json",
      body: JSON.stringify({ detail: "skill_md が不正です" }),
    }),
  );
  await page.getByRole("button", { name: "保存" }).click();
  await expect(page.getByRole("alert")).toHaveText("skill_md が不正です");
  await expect(page.getByLabel("名前")).toHaveValue("new-skill");
  await page.goto(`${gateway.base}/knowledge/skills?name=demo`);
  await expect(page.getByText("登録済み").filter({ visible: true }).first()).toBeVisible();
  await expect(page.getByText("配送なし").filter({ visible: true }).first()).toBeVisible();
  await page.getByRole("button", { name: "削除" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "skill「demo」を削除" }).click();
  await expect
    .poll(
      () =>
        daemon.requests.filter((request) => request.method === "DELETE" && request.path.endsWith("/skills/demo"))
          .length,
    )
    .toBe(1);
});

test("/knowledge fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  for (const fixture of [
    "/knowledge",
    "/knowledge/inbox",
    "/knowledge/skills",
    "/knowledge/skills?create=1",
    "/knowledge/skills?name=demo",
    "/knowledge/skills?name=demo&edit=1",
  ]) {
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}${fixture}`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      if (fixture.includes("edit=1") || fixture.includes("create=1"))
        await expect(page.getByLabel("SKILL.md")).toBeVisible();
      else if (fixture.includes("name=demo")) await expect(page.getByText("Description")).toBeVisible();
      else if (fixture === "/knowledge/inbox") await expect(page.getByRole("button", { name: "採用" })).toBeVisible();
      else if (fixture === "/knowledge") await expect(page.getByRole("link", { name: "Demo knowledge" })).toBeVisible();
      else await expect(page.getByRole("link", { name: "demo" })).toBeVisible();
      await page.screenshot({
        path: path.join(out as string, `knowledge-${fixture.replace(/[^a-z0-9]/gi, "-")}-${width}.png`),
        fullPage: true,
      });
    }
  }
});
