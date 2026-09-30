import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// 案件（R09・R10）の parity。偽 daemon の fixture はこの file の中にまとめる。
function harness(fixtures: Record<string, unknown>) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-projects-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...fixtures } });
  return {
    daemon,
    async start() {
      const daemonUrl = await daemon.start();
      return startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    },
    async close(gateway: Awaited<ReturnType<typeof startGateway>>) {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

const project = (id: string, title: string, archived = false) => ({
  id,
  title,
  request: `${title} の依頼`,
  status: "active",
  created_at: "2026-09-30T00:00:00Z",
  updated_at: "2026-09-30T00:00:00Z",
  ...(archived ? { archived_at: "2026-09-30T01:00:00Z" } : {}),
});

test("parity: /projects 一覧・作成・422 表示", async ({ page }) => {
  const calls: string[] = [];
  const h = harness({
    "/api/v1/projects": (url: URL) => {
      calls.push(url.search);
      return url.searchParams.get("archived")
        ? { items: [project("P1", "一件目"), project("P9", "古い案件", true)] }
        : { items: [project("P1", "一件目")] };
    },
  });
  const gateway = await h.start();
  let posts = 0;
  let submitted: unknown = null;
  await page.route("**/api/projects", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    posts += 1;
    submitted = route.request().postDataJSON();
    await route.fulfill({
      status: posts === 1 ? 422 : 200,
      contentType: "application/json",
      body: JSON.stringify(posts === 1 ? { error: "案件名が空です" } : project("P42", "新しい案件")),
    });
  });
  try {
    await page.goto(`${gateway.base}/projects`);
    await expect(page.locator("[data-project-id='P1']")).toBeVisible();
    await expect(page.locator("[data-project-id='P9']")).toHaveCount(0);
    await page.getByRole("checkbox", { name: "アーカイブした案件も出す" }).click();
    await expect(page.locator("[data-project-id='P9']")).toContainText("アーカイブ済み");
    await expect(page).toHaveURL(/archived=/);
    expect(calls.some((query) => query.includes("archived=1"))).toBe(true);
    await page.getByLabel("案件名").fill("新しい案件");
    await page.getByLabel("依頼").fill("依頼の本文");
    await page.getByLabel("作業場所（手元の path、任意）").fill("/work/p");
    const create = page.getByRole("button", { name: "案件を作成" });
    await create.evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
    await expect(page.getByRole("alert")).toHaveText("案件名が空です");
    expect(posts).toBe(1);
    await expect(page.getByLabel("案件名")).toHaveAttribute("aria-describedby", "project-create-error");
    await expect(page.getByLabel("案件名")).toHaveValue("新しい案件");
    expect(submitted).toEqual({
      title: "新しい案件",
      request: "依頼の本文",
      workspace: { kind: "local", path: "/work/p" },
    });
    await create.click();
    await expect(page).toHaveURL(`${gateway.base}/projects/P42`);
    expect(posts).toBe(2);
  } finally {
    await h.close(gateway);
  }
});
