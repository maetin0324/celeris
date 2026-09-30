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

const detail = {
  project: { ...project("P1", "一件目"), workspace: { kind: "local", path: "/work/p1" } },
  milestones: [],
  root_totals: { root_tasks: 1, by_status: { running: 1 }, totals: {} },
  project_plan: {
    nodes: ["a", "b", "c", "d", "e", "f"].map((key, i) => ({
      key,
      title: `途中目標 ${key} ${"とても長い題".repeat(4)}`,
      depends_on: i === 0 ? [] : [["a", "b", "c", "d", "e", "f"][i - 1]],
      milestone_id: `M${i}`,
      task_id: `T${i + 1}`,
      children_done: 0,
      children_total: 1,
      work_units_done: 0,
      work_units_total: 0,
    })),
  },
  tasks: [
    { id: "T1", title: "根の仕事", status: "running", conversation: false, depends_on: [], is_root_task: true },
    ...Array.from({ length: 8 }, (_, i) => ({
      id: `T${i + 2}`,
      title: `子の仕事 ${i + 2} ${"長い名前".repeat(6)}`,
      status: "ready",
      conversation: false,
      depends_on: [],
      parent_id: `T${i + 1}`,
    })),
  ],
};

test("parity: /projects/:id 表示・計画 DAG・仕事の木・成果物", async ({ page }) => {
  const h = harness({
    "/api/v1/projects/P1": detail,
    "/api/v1/projects": { items: [project("P1", "一件目")] },
    "/api/v1/tasks": { items: [task("X9", "案件外")], total: 1, next_cursor: null, counts_by_status: {} },
    "/api/v1/tasks/T1/artifacts": { items: [] },
  });
  const gateway = await h.start();
  const count = () => h.daemon.requests.filter((r) => r.path === "/api/v1/projects/P1").length;
  try {
    // 案件外の task を一覧で cache に載せてから、アプリ内の遷移で案件詳細へ。
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id='X9']")).toBeVisible();
    await page.evaluate(() => {
      history.pushState({}, "", "/projects");
      dispatchEvent(new PopStateEvent("popstate"));
    });
    await page.locator("[data-project-id='P1'] a").click();
    await expect(page.getByRole("heading", { level: 1, name: "案件の詳細 P1" })).toBeVisible();
    await expect(page.getByTestId("project-overview")).toContainText("/work/p1");
    await expect(page.locator("[data-dag-node]")).toHaveCount(6);
    await expect(page.locator("[data-tree-task='T9']")).toBeVisible();
    // DAG と木は枠の中でスクロールし、ページは横に溢れない。
    for (const id of ["project-dag-frame", "project-tree-frame"]) {
      const frame = page.getByTestId(id);
      expect(await frame.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    // 成果物は開いたときだけ取る。
    await page.getByText("根の仕事 の成果物").click();
    await expect(page.getByTestId("task-artifacts-empty")).toBeVisible();
    await expect.poll(() => h.daemon.streamClients).toBeGreaterThan(0);
    const before = count();
    // 案件に属さない task（一覧で project_id が無いと分かっている）のイベントでは取り直さない。
    h.daemon.sendEvent("task.event", {
      id: 1,
      seq: 1,
      task_id: "X9",
      ts: "2026-09-30T00:00:00Z",
      event: { type: "transitioned", from: "ready", to: "running" },
    });
    await page.waitForTimeout(1500);
    expect(count()).toBe(before);
    // 案件の task のイベントでは取り直す。
    h.daemon.sendEvent("task.event", {
      id: 2,
      seq: 2,
      task_id: "T2",
      ts: "2026-09-30T00:00:00Z",
      event: { type: "transitioned", from: "ready", to: "running" },
    });
    await expect.poll(count).toBeGreaterThan(before);
  } finally {
    await h.close(gateway);
  }
});

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
