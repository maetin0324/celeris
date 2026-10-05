import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// qa-work fix-projects（非 parity）: 文書の保守・board・一覧の critique 指摘（W-06・W-07・W-09・W-23・W-25・W-26・W-30）を本文で確かめる。
function harness(fixtures: Record<string, unknown>) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-work-projects-"));
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

const project = (id: string, title: string) => ({
  id,
  title,
  request: `${title} の依頼`,
  status: "active",
  created_at: "2026-09-30T00:00:00Z",
  updated_at: "2026-09-30T00:00:00Z",
});

const task = (id: string, title: string, extra: Record<string, unknown> = {}) => ({
  id,
  title,
  status: "ready",
  kind: "execute",
  category: "feature",
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
  ...extra,
});

const maintenance = {
  audit: {
    revision: "abc",
    conventions: [],
    documents: [
      { path: "docs/old.md", title: "古い手順", category: "historical", findings: ["2 年更新がない"] },
      { path: "docs/ok.md", title: "正本", category: "canonical", findings: [] },
    ],
  },
  proposal: {
    revision: "abc",
    expected: {},
    actions: [
      { operation: "archive", path: "docs/old.md", destination: "docs/archive/old.md" },
      { operation: "delete", path: "docs/tmp.md" },
    ],
    rationale: ["古い手順を保管へ移す"],
  },
  policy: { mode: "observe" },
};

test("文書の保守: 指摘の一覧・整理案の表・適用は確認を通す・JSON の誤りは欄の alert", async ({ page }) => {
  const h = harness({ "/api/v1/projects/P1/docs/maintenance": maintenance });
  const gateway = await h.start();
  const ops: string[] = [];
  await page.route("**/api/projects/P1/docs/maintenance", async (route) => {
    if (route.request().method() === "GET") return route.fallback();
    ops.push(route.request().postDataJSON()?.op);
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ task_id: "T7" }) });
  });
  try {
    await page.goto(`${gateway.base}/projects/P1/docs/maintenance`);
    const findings = page.getByTestId("docs-audit-findings");
    await expect(findings).toContainText("古い手順");
    await expect(findings).toContainText("2 年更新がない");
    await expect(findings).not.toContainText("正本");
    const plan = page.getByTestId("docs-plan-actions");
    await expect(plan).toContainText("保管へ移す");
    await expect(plan).toContainText("docs/old.md → docs/archive/old.md");
    await expect(plan).toContainText("削除");
    await expect(page.getByTestId("docs-policy")).toContainText("観察のみ");
    // 生の JSON は既定で閉じた details の中。
    await expect(page.getByLabel("整理案 JSON")).toBeHidden();
    // 適用は確認を通す。戻れば送らない。
    await page.getByRole("button", { name: "承認済み案を適用" }).click();
    const dialog = page.getByRole("alertdialog", { name: "整理案を適用しますか" });
    await expect(dialog).toContainText("2 操作・2 file");
    await dialog.getByRole("button", { name: "戻る" }).click();
    await expect(dialog).toHaveCount(0);
    expect(ops).toEqual([]);
    await page.getByRole("button", { name: "承認済み案を適用" }).click();
    await dialog.getByRole("button", { name: "整理案を適用する" }).click();
    await expect.poll(() => ops).toEqual(["apply"]);
    await expect(page.getByRole("link", { name: "結果のタスクを開く" })).toHaveAttribute("href", "/tasks/T7");
    // 読めない JSON は送らず、欄に結び付けた alert を出す。
    await page.getByText("ポリシーの JSON を直す").click();
    await page.getByLabel("ポリシー JSON").fill("{ mode: ");
    await page.getByRole("button", { name: "ポリシーを採用" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "ポリシーを採用する" }).click();
    await expect(page.locator("#docs-policy-error")).toHaveText(/JSON として読めません/);
    await expect(page.getByLabel("ポリシー JSON")).toHaveAttribute("aria-describedby", "docs-policy-error");
    expect(ops).toEqual(["apply"]);
  } finally {
    await h.close(gateway);
  }
});

test("文書の保守: 文書リポジトリが無い（409）ときは文書画面へ案内する", async ({ page }) => {
  const h = harness({});
  const gateway = await h.start();
  await page.route("**/api/projects/P1/docs/maintenance", (route) =>
    route.fulfill({
      status: 409,
      contentType: "application/json",
      body: JSON.stringify({ error: "docs_unavailable" }),
    }),
  );
  try {
    await page.goto(`${gateway.base}/projects/P1/docs/maintenance`);
    await expect(page.getByText("文書リポジトリがありません")).toBeVisible();
    await expect(page.getByRole("link", { name: "文書を用意する" })).toHaveAttribute("href", "/projects/P1/docs");
  } finally {
    await h.close(gateway);
  }
});

test("board: すべての案件では行に案件名、内部の英語値は日本語", async ({ page }) => {
  const h = harness({
    "/api/v1/projects": { items: [project("P1", "一件目の案件"), project("P2", "二件目")] },
    "/api/v1/tasks": {
      items: [
        task("T1", "カード 1", { project_id: "P1" }),
        task("T2", "カード 2", { project_id: "P2", tier: "frontier" }),
      ],
      total: 2,
      next_cursor: null,
      counts_by_status: {},
    },
  });
  const gateway = await h.start();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/board`);
    await expect(page.locator("[data-task-id='T1'] [data-testid='board-row-project']")).toHaveText(
      "案件: 一件目の案件",
    );
    await expect(page.locator("[data-task-id='T2']")).toContainText("最上位");
    await expect(page.locator("[data-task-id='T1']")).toContainText("標準");
    await expect(page.locator("[data-task-id='T1']")).toContainText("機能");
    await expect(page.locator("[data-task-id='T1']")).toContainText("通常");
    for (const raw of ["standard", "frontier", "feature", "normal"]) {
      await expect(page.getByTestId("board-table")).not.toContainText(raw);
    }
    // 案件で絞ったときは案件名を繰り返さない。
    await page.goto(`${gateway.base}/board?project=P1`);
    await expect(page.locator("[data-task-id='T1']")).toBeVisible();
    await expect(page.getByTestId("board-row-project")).toHaveCount(0);
  } finally {
    await h.close(gateway);
  }
});

test("案件一覧: 360 幅で横に溢れず、判断待ちは案件で絞った受信箱へ", async ({ page }) => {
  const h = harness({ "/api/v1/projects": { items: [project("P1", "一件目")] } });
  const gateway = await h.start();
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/projects`);
    const row = page.locator("[data-project-id='P1']");
    await expect(row).toBeVisible();
    await expect(row).toContainText("途中目標");
    expect(
      await page.getByTestId("projects-list").evaluate((el) => {
        const frame = el.parentElement;
        return !!frame && frame.scrollWidth <= frame.clientWidth;
      }),
    ).toBe(true);
    const pending = row.getByTestId("project-pending").getByRole("link");
    if ((await pending.count()) > 0) await expect(pending).toHaveAttribute("href", "/inbox?project=P1");
  } finally {
    await h.close(gateway);
  }
});
