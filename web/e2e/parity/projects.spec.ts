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
    // 一覧は card ではなく table（状態・途中目標・判断待ち・最終更新）。判断待ちは受信箱の項目の project_id から数える。
    const table = page.getByRole("table");
    for (const name of ["案件", "状態", "途中目標", "判断待ち", "最終更新"]) {
      await expect(table.getByRole("columnheader", { name, exact: true })).toBeVisible();
    }
    await expect(page.locator("[data-project-id='P1'] [data-testid='project-pending']")).toHaveText("2 件");
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
    await page.locator("[data-project-id='P1']").getByRole("link", { name: "一件目" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "案件の詳細 P1" })).toBeVisible();
    await expect(page.getByTestId("project-overview")).toContainText("/work/p1");
    await expect(page.locator("[data-dag-node]")).toHaveCount(6);
    await expect(page.locator("[data-tree-task='T9']")).toBeVisible();
    // 案件 → 途中目標 → task の木: 根の仕事は計画の節点の途中目標の下、子は字下げの深さを持つ。
    const milestone = page.locator("[data-milestone='M0']");
    await expect(milestone.getByRole("columnheader").first()).toContainText("途中目標 1: 途中目標 a");
    await expect(milestone.locator("[data-tree-task='T1']")).toHaveAttribute("data-depth", "0");
    await expect(milestone.locator("[data-tree-task='T9']")).toHaveAttribute("data-depth", "8");
    await expect(milestone.locator("[data-tree-task='T2'] [data-status='ready']")).toHaveText("実行待ち");
    // 判断待ちのある task は受信箱へ link する（偽 daemon の受信箱は T1・T2 を止めている）。
    await expect(milestone.locator("[data-tree-task='T1']").getByTestId("tree-task-pending")).toHaveAttribute(
      "href",
      "/inbox",
    );
    // DAG は枠の中でスクロールし、ページは横に溢れない。木は 640px 未満で行を積み、枠にも収まる（fix-r5 fix-narrow）。
    const dag = page.getByTestId("project-dag-frame");
    expect(await dag.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
    const tree = page.getByTestId("project-tree-frame");
    expect(await tree.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
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

// P4-03〜P4-05: 案件詳細の操作。書き込みは page.route で受け、送った method・path・本文を確かめる。
type Sent = { method: string; path: string; body: unknown };
async function captureWrites(page: import("@playwright/test").Page) {
  const sent: Sent[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    if (request.method() === "GET") return route.fallback();
    sent.push({ method: request.method(), path: new URL(request.url()).pathname, body: request.postDataJSON() });
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  return sent;
}

const opsDetail = {
  ...detail,
  milestones: [],
  milestones_frozen: 0,
  repos: [
    {
      id: "R1",
      name: "main-repo",
      kind: "git",
      project_id: "P1",
      is_primary: true,
      location: { kind: "local", path: "/work/p1" },
      created_at: "2026-09-30T00:00:00Z",
    },
    {
      id: "R2",
      name: "sub-repo",
      kind: "git",
      project_id: "P1",
      location: { kind: "local", path: "/work/p1-sub" },
      created_at: "2026-09-30T00:00:00Z",
    },
  ],
};

async function openDetail(page: import("@playwright/test").Page, fixtures: Record<string, unknown> = {}) {
  const h = harness({ "/api/v1/projects/P1": opsDetail, "/api/v1/tasks/T1/artifacts": { items: [] }, ...fixtures });
  const gateway = await h.start();
  const sent = await captureWrites(page);
  await page.goto(`${gateway.base}/projects/P1`);
  await expect(page.getByRole("heading", { level: 1, name: "案件の詳細 P1" })).toBeVisible();
  return { h, gateway, sent };
}

const last = (sent: Sent[]) => sent.at(-1);

test("parity: /projects/:id 案件の操作", async ({ page }) => {
  const { h, gateway, sent } = await openDetail(page);
  try {
    const ops = page.getByTestId("project-ops");
    await ops.getByLabel("案件名").fill("改名した案件");
    await ops.getByRole("button", { name: "名前と依頼を保存" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ method: "PATCH", path: "/api/projects/P1", body: { title: "改名した案件" } });
    await ops.getByLabel("状態").selectOption("done");
    await ops.getByRole("button", { name: "状態を変える" }).click();
    await expect.poll(() => last(sent)).toMatchObject({ method: "PATCH", body: { status: "done" } });
    await ops.getByRole("button", { name: "一時停止" }).click();
    await expect.poll(() => last(sent)?.path).toBe("/api/projects/P1/pause");
    // 中止とアーカイブは確認のダイアログ（ConfirmDialog）を通す。やめれば送らない。
    const before = sent.length;
    await ops.getByRole("button", { name: "中止", exact: true }).click();
    const cancelDialog = page.getByRole("alertdialog", { name: "案件を中止しますか" });
    await expect(cancelDialog).toContainText("対象: 一件目");
    await cancelDialog.getByRole("button", { name: "やめる" }).click();
    await expect(cancelDialog).toHaveCount(0);
    expect(sent.length).toBe(before);
    await ops.getByRole("button", { name: "中止", exact: true }).click();
    await cancelDialog.getByRole("button", { name: "案件を中止する" }).click();
    await expect.poll(() => last(sent)?.path).toBe("/api/projects/P1/cancel");
    await expect(cancelDialog).toHaveCount(0);
    await ops.getByRole("button", { name: "アーカイブ", exact: true }).click();
    await page
      .getByRole("alertdialog", { name: "案件をアーカイブしますか" })
      .getByRole("button", { name: "案件をアーカイブする" })
      .click();
    await expect.poll(() => last(sent)?.path).toBe("/api/projects/P1/archive");
    await ops.getByLabel("作業場所の path").fill("/work/new");
    await ops.getByRole("button", { name: "作業場所を保存" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ method: "PATCH", body: { workspace: { kind: "local", path: "/work/new" } } });
    await ops.getByRole("button", { name: "作業場所を消す" }).click();
    await expect.poll(() => last(sent)).toMatchObject({ method: "PATCH", body: { workspace: null } });
    await ops.getByLabel("仕事の題").fill("足す仕事");
    await ops.getByRole("button", { name: "仕事を足す" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ method: "POST", path: "/api/tasks", body: { title: "足す仕事", project_id: "P1" } });
    await expect(ops.getByRole("status").first()).toHaveText("操作が完了しました");
  } finally {
    await h.close(gateway);
  }
});

test("parity: /projects/:id 計画と途中目標", async ({ page }) => {
  const { h, gateway, sent } = await openDetail(page);
  const count = () => h.daemon.requests.filter((r) => r.path === "/api/v1/projects/P1").length;
  try {
    const ops = page.getByTestId("project-plan-ops");
    await ops.getByLabel("計画の目標").fill("計画の目標文");
    await ops.getByLabel("段階（1 行に 1 つ、任意）").fill("調べる\n作る");
    await ops.getByRole("button", { name: "計画を立てる" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({
        path: "/api/tasks",
        body: { project_id: "P1", stages_hint: [{ title: "調べる" }, { title: "作る" }] },
      });
    await ops.getByLabel("途中目標").fill("中間の目標");
    await ops.getByRole("button", { name: "途中目標を足す" }).click();
    await expect.poll(() => last(sent)).toMatchObject({ body: { stages_hint: [{ title: "中間の目標" }] } });
    const row = ops.locator("[data-root-task='T1']");
    await row.getByRole("button", { name: "計画を承認" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ path: "/api/tasks/T1/execution/plan-gate", body: { action: "approve" } });
    await row.getByRole("button", { name: "段階を通す" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ path: "/api/tasks/T1/execution/phase-gate", body: { action: "continue" } });
    // 取り下げと取り消しは確認を通す。
    await row.getByRole("button", { name: "段階を取り下げる" }).click();
    await page
      .getByRole("alertdialog", { name: "段階を取り下げますか" })
      .getByRole("button", { name: "段階を取り下げる" })
      .click();
    await expect.poll(() => last(sent)).toMatchObject({ body: { action: "withdraw" } });
    for (const [label, action] of [
      ["止める", "pause"],
      ["再開", "resume"],
    ] as const) {
      await row.getByRole("button", { name: label, exact: true }).click();
      await expect.poll(() => last(sent)?.path).toBe(`/api/tasks/T1/${action}`);
    }
    const beforeCancel = sent.length;
    await row.getByRole("button", { name: "取り消す", exact: true }).click();
    const cancelMilestone = page.getByRole("alertdialog", { name: "途中目標の仕事を取り消しますか" });
    await cancelMilestone.getByRole("button", { name: "やめる" }).click();
    expect(sent.length).toBe(beforeCancel);
    await row.getByRole("button", { name: "取り消す", exact: true }).click();
    await cancelMilestone.getByRole("button", { name: "仕事を取り消す" }).click();
    await expect.poll(() => last(sent)?.path).toBe("/api/tasks/T1/cancel");
    // project_plan_proposed / decided でその project の detail を取り直す。
    await expect.poll(() => h.daemon.streamClients).toBeGreaterThan(0);
    for (const [i, kind] of (["project_plan_proposed", "project_plan_decided"] as const).entries()) {
      const before = count();
      h.daemon.sendEvent("task.event", {
        id: 100 + i,
        seq: 100 + i,
        task_id: "T1",
        ts: "2026-09-30T00:00:00Z",
        event: { type: kind, project_id: "P1", version: 2 },
      });
      await expect.poll(count).toBeGreaterThan(before);
    }
  } finally {
    await h.close(gateway);
  }
});

test("parity: /projects/:id 全 intent・計画・木", async ({ page }) => {
  const { h, gateway, sent } = await openDetail(page);
  try {
    const repos = page.getByTestId("project-repos");
    await expect(repos.locator("[data-repo]")).toHaveCount(2);
    await repos.getByLabel("リポジトリ名").fill("new-repo");
    await repos.getByLabel("リポジトリの path").fill("/work/new-repo");
    await repos.getByRole("button", { name: "リポジトリを足す" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({
        method: "POST",
        path: "/api/projects/P1/repos",
        body: { name: "new-repo", location: { kind: "local", path: "/work/new-repo" } },
      });
    const r2 = repos.locator("[data-repo='R2']");
    await r2.getByLabel("既定ブランチ R2").fill("develop");
    await r2.getByRole("button", { name: "保存" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ method: "PATCH", path: "/api/repos/R2", body: { default_branch: "develop" } });
    await r2.getByRole("button", { name: "主にする" }).click();
    await expect
      .poll(() => last(sent))
      .toMatchObject({ method: "PATCH", path: "/api/repos/R2", body: { is_primary: true } });
    await r2.getByRole("button", { name: "削除", exact: true }).click();
    await page
      .getByRole("alertdialog", { name: "リポジトリを案件から外しますか" })
      .getByRole("button", { name: "sub-repo を外す" })
      .click();
    await expect.poll(() => last(sent)).toMatchObject({ method: "DELETE", path: "/api/repos/R2" });
    // 案件・計画の intent も同じ画面にあり、計画の DAG と仕事の木も出ている。
    await expect(page.getByTestId("project-ops")).toBeVisible();
    await expect(page.getByTestId("project-plan-ops")).toBeVisible();
    await expect(page.locator("[data-dag-node]")).toHaveCount(6);
    await expect(page.locator("[data-tree-task='T9']")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally {
    await h.close(gateway);
  }
});

// P4-06: document fixtures and mutations stay in this describe to avoid parallel fixture edits.
test.describe("P4-06 docs", () => {
  const tree = {
    project_id: "P1",
    repo: "demo",
    root: "docs",
    default_branch: "main",
    truncated: false,
    items: [{ path: "docs/readme.md", title: "案内" }],
  };
  const pageFixture = {
    project_id: "P1",
    repo: "demo",
    root: "docs",
    default_branch: "main",
    path: "docs/readme.md",
    title: "案内",
    raw: "# 案内\n\n元の文章",
    html: "",
    history: [],
    too_large: false,
    etag: "v1",
  };
  test("parity: /projects/:id/docs 初期化・保存・削除", async ({ page }) => {
    const h = harness({ "/api/v1/projects/P1/docs": tree, "/api/v1/projects/P1/docs/page": pageFixture });
    const gateway = await h.start();
    const methods: string[] = [];
    await page.route("**/api/projects/P1/docs/**", async (route) => {
      if (route.request().method() === "GET") return route.fallback();
      methods.push(route.request().method());
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          path: "docs/readme.md",
          deleted: route.request().method() === "DELETE",
          unchanged: false,
          project_id: "P1",
          repo: "demo",
          sha: "v2",
        }),
      });
    });
    try {
      await page.goto(`${gateway.base}/projects/P1/docs?path=docs%2Freadme.md&edit=1`);
      const editor = page.getByTestId("docs-editor");
      await expect(editor).toBeVisible();
      await expect(editor.getByLabel("Markdown")).toHaveValue("# 案内\n\n元の文章");
      await editor.getByLabel("Markdown").fill("# 案内\n\n編集中の文");
      h.daemon.sendEvent("project.event", { project_id: "P1", event: { type: "project_updated" } });
      await page.waitForTimeout(300);
      await expect(editor.getByLabel("Markdown")).toHaveValue("# 案内\n\n編集中の文");
      expect(await editor.getByLabel("Markdown").evaluate((el) => el.getBoundingClientRect().right <= innerWidth)).toBe(
        true,
      );
      await editor.getByRole("button", { name: "保存" }).click();
      await expect(editor.getByRole("status").filter({ hasText: "操作が完了しました" })).toBeVisible();
      await page.getByRole("link", { name: "案内" }).click();
      page.once("dialog", (dialog) => dialog.accept());
      await page.getByRole("button", { name: "削除" }).click();
      expect(methods).toEqual(["PUT", "DELETE"]);
    } finally {
      await h.close(gateway);
    }
  });
  test("docs_unavailable から初期化する", async ({ page }) => {
    const h = harness({});
    const gateway = await h.start();
    let initialized = false;
    await page.route("**/api/projects/P1/docs", async (route) => {
      await route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({ error: "docs_unavailable" }),
      });
    });
    await page.route("**/api/projects/P1/docs/init", async (route) => {
      initialized = true;
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          project_id: "P1",
          repo: "demo",
          root: "docs",
          default_branch: "main",
          path: "/tmp/demo",
          created: true,
        }),
      });
    });
    try {
      await page.goto(`${gateway.base}/projects/P1/docs`);
      await page.getByRole("button", { name: "文書を用意する" }).click();
      await expect(page.getByRole("status").filter({ hasText: "操作が完了しました" })).toBeVisible();
      expect(initialized).toBe(true);
    } finally {
      await h.close(gateway);
    }
  });
  test("parity: /projects/:id/docs/maintenance 起動と結果", async ({ page }) => {
    const h = harness({
      "/api/v1/projects/P1/docs/maintenance": {
        audit: { count: 1 },
        proposal: { actions: [] },
        policy: { mode: "observe" },
      },
    });
    const gateway = await h.start();
    await page.route("**/api/projects/P1/docs/maintenance", async (route) => {
      if (route.request().method() === "GET") return route.fallback();
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ task_id: "T1", op: route.request().postDataJSON()?.op }),
      });
    });
    try {
      await page.goto(`${gateway.base}/projects/P1/docs/maintenance`);
      await expect(page.getByTestId("docs-maintenance")).toBeVisible();
      await page.getByRole("button", { name: "監査結果を保存" }).click();
      await expect(page.getByRole("link", { name: "結果のタスクを開く" })).toHaveAttribute("href", "/tasks/T1");
    } finally {
      await h.close(gateway);
    }
  });
});

test.describe("P4-07 board", () => {
  test("parity: /board 列・URL 絞り込み・編集・復帰", async ({ page }) => {
    const calls: string[] = [];
    const items = ["ready", "running", "blocked", "done", "failed", "cancelled"].map((status, i) => ({
      ...task(`T${i + 1}`, `カード ${i + 1}`),
      actions: ["edit"],
      status,
      project_id: "P1",
    }));
    const h = harness({
      "/api/v1/projects": { items: [project("P1", "一件目")] },
      "/api/v1/tasks": (url: URL) => {
        calls.push(url.search);
        return { items, total: 6, next_cursor: null, counts_by_status: {} };
      },
    });
    const gateway = await h.start();
    let editBody: unknown = null;
    await page.route("**/api/tasks/T1", async (route) => {
      if (route.request().method() !== "PATCH") return route.fallback();
      editBody = route.request().postDataJSON();
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ task: items[0], fields: ["priority"] }),
      });
    });
    try {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.goto(`${gateway.base}/board?project=P1`);
      await expect(page.locator("[data-board-column]")).toHaveCount(6);
      await expect(page.locator("[data-task-id]")).toHaveCount(6);
      // card wall でなく 1 行 1 task の表。スマホ幅でも表もページも横に溢れない（web ADR D5）。
      await expect(page.getByRole("table")).toHaveCount(1);
      expect(
        await page.getByTestId("board-table").evaluate((el) => {
          const frame = el.parentElement;
          return !!frame && frame.scrollWidth <= frame.clientWidth;
        }),
      ).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      // 状態の絞り込みは URL に残り、件数つきで表の上にある。
      await page
        .getByRole("navigation", { name: "状態で絞り込む" })
        .getByRole("link", { name: /停止中/ })
        .click();
      await expect(page).toHaveURL(/project=P1.*column=blocked/);
      await expect(page.locator("[data-task-id]")).toHaveCount(1);
      await page
        .getByRole("navigation", { name: "状態で絞り込む" })
        .getByRole("link", { name: /すべて/ })
        .click();
      await expect(page.locator("[data-task-id]")).toHaveCount(6);
      await page.getByLabel("検索").fill("カード");
      await page.getByRole("button", { name: "絞り込む" }).click();
      await expect(page).toHaveURL(/project=P1.*q=/);
      expect(calls.some((q) => q.includes("project=P1") && q.includes("q="))).toBe(true);
      await page.locator("[data-task-id='T1']").getByRole("button", { name: "カード 1 を編集" }).click();
      const edit = page.getByRole("form", { name: "カード 1 を編集" });
      await edit.getByLabel("優先度").selectOption("P0");
      await edit.getByRole("button", { name: "保存" }).click();
      await expect(page.locator("[data-task-edit='T1'] [role=status]")).toBeVisible();
      expect(editBody).toMatchObject({ priority: "P0", expected_status: "ready" });
      await page.goBack();
      await expect(page).toHaveURL(/project=P1/);
    } finally {
      await h.close(gateway);
    }
  });
});
