import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

function harness(fixtures: Record<string, unknown>) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-tasks-"));
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

const task = (id: string, title: string) => ({
  id,
  title,
  status: "ready",
  kind: "execute",
  category: "work",
  tier: "standard",
  actions: [],
  attempts: 0,
  children: 0,
  conversation: false,
  created_at: "2026-09-30T00:00:00Z",
  depends_on: [],
  labels: [],
  max_retries: 0,
  pending_children: 0,
  priority: 0,
  priority_label: "normal",
  updated_at: "2026-09-30T00:00:00Z",
});

test("parity: /tasks 絞り込み・検索・続き", async ({ page }) => {
  const calls: string[] = [];
  const fixture = (url: URL) => {
    calls.push(url.search);
    const cursor = url.searchParams.get("cursor");
    return cursor
      ? { items: [task("T2", "二件目")], total: 2, next_cursor: null, counts_by_status: { ready: 2 } }
      : { items: [task("T1", "一件目")], total: 2, next_cursor: "next", counts_by_status: { ready: 2 } };
  };
  const h = harness({ "/api/v1/tasks": fixture });
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id='T1']")).toBeVisible();
    await page.getByRole("button", { name: "さらに読む" }).click();
    await expect(page.locator("[data-task-id='T2']")).toBeVisible();
    h.daemon.sendEvent("task.event", { cursor: 1, event: { id: 1, task_id: "T1", event: { type: "task_updated" } } });
    await expect(page.locator("[data-task-id='T2']")).toBeVisible();
    const search = page.getByRole("searchbox", { name: "検索" });
    await search.fill("計算");
    await page.getByRole("checkbox", { name: "ready" }).check();
    await page.getByLabel("並び").selectOption("created_desc");
    await page.locator("[data-testid='tasks-filter'] select[name='limit']").selectOption("20");
    await search.focus();
    await search.press("Enter");
    await expect(page).toHaveURL(/q=%E8%A8%88%E7%AE%97/);
    await expect(page).toHaveURL(/status=ready/);
    await expect(page).toHaveURL(/order=created_desc/);
    await expect(page).toHaveURL(/limit=20/);
    await expect(search).toBeFocused();
    await expect
      .poll(() => calls.some((query) => query.includes("q=%E8%A8%88%E7%AE%97") && query.includes("limit=20")))
      .toBe(true);
    await page.reload();
    await expect(page.getByRole("searchbox", { name: "検索" })).toHaveValue("計算");
    await expect(page.getByRole("checkbox", { name: "ready" })).toBeChecked();
    await expect(page).toHaveURL(/status=ready/);
    await page.getByRole("searchbox", { name: "検索" }).fill("別条件");
    await page.getByRole("searchbox", { name: "検索" }).press("Enter");
    await page.goBack();
    await expect(page.getByRole("searchbox", { name: "検索" })).toHaveValue("計算");
    expect(calls.some((query) => query.includes("cursor=next"))).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(
      0,
    );
  } finally {
    await h.close(gateway);
  }
});

test("parity: /graph root・depth", async ({ page }) => {
  const calls: string[] = [];
  const graph = {
    nodes: [
      { id: "T1", title: "親", kind: "execute", status: "ready" },
      { id: "T2", title: "子", kind: "execute", status: "running", parent_id: "T1" },
    ],
    edges: [{ from: "T1", to: "T2", kind: "depends_on" }],
  };
  const h = harness({
    "/api/v1/graph": (url: URL) => {
      calls.push(url.search);
      return graph;
    },
  });
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/graph`);
    await expect(page.locator("[data-graph-node]")).toHaveCount(2);
    await page.getByLabel("root").fill("T1");
    await page.getByLabel("depth").fill("2");
    await page.getByRole("button", { name: "絞り込み" }).click();
    await expect(page).toHaveURL(/root=T1&depth=2/);
    await expect.poll(() => calls.some((query) => query.includes("root=T1") && query.includes("depth=2"))).toBe(true);
    await page.getByLabel("root").fill("T2");
    await page.getByRole("button", { name: "絞り込み" }).click();
    await page.goBack();
    await expect(page.getByLabel("root")).toHaveValue("T1");
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(
      0,
    );
    expect(await page.locator("[data-testid='graph-canvas']").evaluate((el) => el.scrollWidth >= el.clientWidth)).toBe(
      true,
    );
  } finally {
    await h.close(gateway);
  }
});

test("parity: /tasks/new 作成・条件 4 型・422", async ({ page }) => {
  const h = harness({});
  const gateway = await h.start();
  let requests = 0;
  let submitted: Record<string, unknown> | null = null;
  await page.route("**/api/tasks", async (route) => {
    requests += 1;
    submitted = route.request().postDataJSON() as Record<string, unknown>;
    await route.fulfill({
      status: requests === 1 ? 422 : 200,
      contentType: "application/json",
      body: JSON.stringify(requests === 1 ? { error: "名前を確認してください" } : { id: "T42" }),
    });
  });
  try {
    await page.goto(`${gateway.base}/tasks/new`);
    await page.getByLabel("名前").fill("新しいタスク");
    await page.getByLabel("目的").fill("目的の本文");
    const rows = page.locator("[data-testid='criterion-row']");
    await rows.nth(0).getByRole("combobox").selectOption("command");
    await rows.nth(0).getByRole("textbox").fill("cargo test");
    for (const [type, value] of [
      ["artifact_exists", "result.txt"],
      ["reviewer", "reviewer check"],
      ["human", "human check"],
    ]) {
      await page.getByRole("button", { name: "条件を追加" }).click();
      const row = rows.last();
      await row.getByRole("combobox").selectOption(type);
      await row.getByRole("textbox").fill(value);
    }
    const create = page.getByRole("button", { name: "タスクを作成" });
    await create.evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
    await expect.poll(() => requests).toBe(1);
    await expect(page.getByRole("alert")).toHaveText("名前を確認してください");
    expect(requests).toBe(1);
    await expect(page.getByLabel("名前")).toHaveValue("新しいタスク");
    expect(submitted).toEqual({
      title: "新しいタスク",
      objective: "目的の本文",
      acceptance: [
        { type: "command", cmd: "cargo test", expect_exit: 0 },
        { type: "artifact_exists", name: "result.txt" },
        { type: "reviewer", text: "reviewer check" },
        { type: "human", text: "human check" },
      ],
    });
    await rows.nth(3).getByRole("button", { name: "条件を削除" }).click();
    await expect(rows).toHaveCount(3);
    await page.getByRole("button", { name: "条件を追加" }).click();
    await create.click();
    await expect(page).toHaveURL(`${gateway.base}/tasks/T42`);
    expect(requests).toBe(2);
    expect(await rows.count()).toBe(0);
  } finally {
    await h.close(gateway);
  }
});

test("parity: /plans/new 作成と失敗表示", async ({ page }) => {
  const h = harness({});
  const gateway = await h.start();
  let requests = 0;
  let submitted: unknown;
  await page.route("**/api/plans", async (route) => {
    requests += 1;
    submitted = route.request().postDataJSON();
    await route.fulfill({
      status: requests === 1 ? 422 : 200,
      contentType: "application/json",
      body: JSON.stringify(requests === 1 ? { error: "目標を確認してください" } : { id: "T43" }),
    });
  });
  try {
    await page.goto(`${gateway.base}/plans/new`);
    await page.getByLabel("目標").fill("大きな目標");
    const create = page.getByRole("button", { name: "計画を作成" });
    await create.evaluate((button) => {
      (button as HTMLButtonElement).click();
      (button as HTMLButtonElement).click();
    });
    await expect.poll(() => requests).toBe(1);
    await expect(page.getByRole("alert")).toHaveText("目標を確認してください");
    await expect(page.getByLabel("目標")).toHaveValue("大きな目標");
    expect(requests).toBe(1);
    expect(submitted).toEqual({ goal: "大きな目標" });
    await create.click();
    await expect(page).toHaveURL(`${gateway.base}/tasks/T43`);
    expect(requests).toBe(2);
  } finally {
    await h.close(gateway);
  }
});
