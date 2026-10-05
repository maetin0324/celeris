import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// qa-work fix-reports（W-10〜12・W-31〜36・W-48〜50）で直した /reports・/approvals の振る舞いを固定する（parity 外）。
test.describe.configure({ mode: "default" });

const dir = mkdtempSync(path.join(tmpdir(), "celeris-work-reports-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const base = { level: 2, created_at: "2026-10-01T00:00:00Z", read_at: null, sources: [] };
const reports = [
  { ...base, id: "R1", headline: "ビルドが壊れた", kind: "bad_news", node_id: "web", task_id: "T1", project_id: "P1" },
  { ...base, id: "R2", headline: "方式を選んでほしい", kind: "question", node_id: "web", level: 1 },
  { ...base, id: "R3", headline: "週次のまとめ", kind: "result", node_id: "unknown-node-0123456789abcdef" },
];
const org = {
  items: [
    { id: "web", name: "Web 課", kind: "section", created_at: base.created_at, updated_at: base.created_at },
    { id: "eng", name: "開発部", kind: "department", created_at: base.created_at, updated_at: base.created_at },
  ],
};
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: {
    ...defaultFixtures,
    "/api/v1/reports": { items: reports },
    "/api/v1/reports/R1": { report: { ...reports[0], body: "## 原因\n\n`cargo build` が失敗" }, sources_expanded: [] },
    "/api/v1/org": org,
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

const routeOrg = (page: Page) =>
  page.route("**/api/org", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(org) }),
  );

test("/reports: 種類は和名と色、送り手と元タスク、展開の名前", async ({ page }) => {
  await routeOrg(page);
  await page.goto(`${gateway.base}/reports`);
  const list = page.getByRole("list", { name: "報告の一覧" });
  const r1 = list.locator('[data-report-id="R1"]');
  await expect(r1.getByText("悪い知らせ")).toHaveAttribute("data-tone", "danger");
  await expect(list.locator('[data-report-id="R2"]').getByText("質問")).toHaveAttribute("data-tone", "warning");
  await expect(list.getByText("bad_news")).toHaveCount(0);
  await expect(r1.getByText("送り手: Web 課（課）")).toBeVisible();
  await expect(list.locator('[data-report-id="R2"]').getByText("送り手: Web 課（部）")).toBeVisible();
  await expect(r1.getByRole("link", { name: "元のタスクを開く" })).toHaveAttribute("href", /\/tasks\/T1$/);
  await expect(r1.getByRole("link", { name: "案件を開く" })).toHaveAttribute("href", /\/projects\/P1$/);
  await expect(list.locator('[data-report-id="R2"]').getByRole("link", { name: "受信箱で答える" })).toBeVisible();
  // 名前の分からない送り手は省略した ID（全文は読み上げ）。
  await expect(list.locator('[data-report-id="R3"]').getByText("unknown-node-0123456789abcdef")).toBeAttached();
  // 開閉 button は行ごとに名前が違い、本文の領域を指す。
  const toggle = r1.getByRole("button", { name: "「ビルドが壊れた」を展開" });
  const controls = await toggle.getAttribute("aria-controls");
  expect(controls).toBeTruthy();
  await toggle.click();
  await expect(r1.getByRole("button", { name: "「ビルドが壊れた」を閉じる" })).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator(`[id="${controls}"]`).getByText("cargo build")).toBeVisible();
  // 段の絞り込みは行の表示と同じ語。
  const select = page.getByLabel("報告元の段");
  await expect(select.locator("option")).toHaveText(["すべて", "CoS", "部", "課"]);
});

const approvals = [
  {
    id: "A1",
    node_id: "web",
    question: "古い依頼",
    created_at: "2026-09-01T00:00:00Z",
    decided_at: "2026-09-02T00:00:00Z",
    decision: "once",
    task_id: "T1",
  },
  {
    id: "A2",
    node_id: "web",
    question: "常設の依頼",
    created_at: "2026-09-03T00:00:00Z",
    decided_at: "2026-09-05T00:00:00Z",
    decision: "standing",
    answer: "以後も可",
  },
  {
    id: "A3",
    node_id: "eng",
    question: "取り下げた依頼",
    created_at: "2026-09-04T00:00:00Z",
    decided_at: "2026-09-04T01:00:00Z",
    decision: "withdrawn",
  },
];

async function routeApprovals(page: Page, rules: { status: number; items?: unknown[] }) {
  const calls: { method: string; body?: string; url: string }[] = [];
  await routeOrg(page);
  await page.route("**/api/approvals?**", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: approvals }) }),
  );
  await page.route("**/api/standing-rules**", async (route) => {
    const method = route.request().method();
    calls.push({ method, body: route.request().postData() ?? undefined, url: route.request().url() });
    if (method === "POST") {
      const created = { id: "S9", rule: "新しい規則", node_id: "web", created_at: "2026-10-05T00:00:00Z" };
      rules.items = [...(rules.items ?? []), created];
      return route.fulfill({ status: 201, contentType: "application/json", body: JSON.stringify(created) });
    }
    if (method === "DELETE") {
      rules.items = (rules.items ?? []).filter(
        (item) =>
          !route
            .request()
            .url()
            .endsWith((item as { id: string }).id),
      );
      return route.fulfill({ status: 204, body: "" });
    }
    if (rules.status !== 200)
      return route.fulfill({
        status: rules.status,
        contentType: "application/json",
        body: JSON.stringify({ detail: "x" }),
      });
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: rules.items ?? [] }),
    });
  });
  return calls;
}

test("/approvals: 決めた順・結果の色・出所、常設ルールの追加と取り消し", async ({ page }) => {
  const calls = await routeApprovals(page, { status: 200, items: [] });
  await page.goto(`${gateway.base}/approvals`);
  const history = page.getByRole("list", { name: "決めた認可" });
  await expect(history.locator("[data-approval-id]")).toHaveCount(3);
  // 新しく決めた順。
  expect(
    await history
      .locator("[data-approval-id]")
      .evaluateAll((rows) => rows.map((row) => row.getAttribute("data-approval-id"))),
  ).toEqual(["A2", "A3", "A1"]);
  const a2 = history.locator('[data-approval-id="A2"]');
  await expect(a2.locator("[data-slot=badge]")).toHaveAttribute("data-tone", "warning");
  await expect(a2.getByRole("link", { name: "常設ルールを見る" })).toHaveAttribute("href", "#standing-rules");
  await expect(a2.getByText("回答: 以後も可")).toBeVisible();
  await expect(history.locator('[data-approval-id="A3"]').locator("[data-slot=badge]")).toHaveAttribute(
    "data-tone",
    "neutral",
  );
  await expect(history.locator('[data-approval-id="A3"]').getByText("依頼元: 開発部")).toBeVisible();
  await expect(
    history.locator('[data-approval-id="A1"]').getByRole("link", { name: "元のタスクを開く" }),
  ).toHaveAttribute("href", /\/tasks\/T1$/);
  // 対象は組織から選び、追加の前に自動で認められる範囲を読める。
  await page.getByLabel("対象の課（空は全員）").selectOption("web");
  await expect(page.getByText("追加すると、Web 課の認可の依頼のうち")).toBeVisible();
  await page.getByLabel("規則文").fill("新しい規則");
  await page.getByRole("button", { name: "追加", exact: true }).click();
  await expect
    .poll(() => calls.filter((call) => call.method === "POST").map((call) => JSON.parse(call.body ?? "{}")))
    .toEqual([{ rule: "新しい規則", node_id: "web" }]);
  // 追加した直後はその場で取り消せる。
  await page.getByRole("button", { name: "いま足した規則を取り消す" }).click();
  await expect
    .poll(() => calls.filter((call) => call.method === "DELETE").map((call) => call.url))
    .toEqual([expect.stringMatching(/\/standing-rules\/S9$/)]);
  await expect(page.getByRole("button", { name: "いま足した規則を取り消す" })).toHaveCount(0);
});

test("/approvals: 常設ルールを確かめられない間は追加を止める", async ({ page }) => {
  const calls = await routeApprovals(page, { status: 403 });
  await page.goto(`${gateway.base}/approvals`);
  await expect(
    page.getByText("常設ルールを取得する権限", { exact: false }).or(page.getByText("権限がありません").first()),
  ).toBeVisible();
  await expect(page.getByText("既存のルールを確認できないため、追加を止めています。")).toBeVisible();
  await expect(page.getByLabel("規則文")).toBeDisabled();
  await expect(page.getByRole("button", { name: "追加", exact: true })).toBeDisabled();
  expect(calls.filter((call) => call.method === "POST")).toHaveLength(0);
});
