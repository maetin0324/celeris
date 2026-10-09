import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { seriousViolations } from "../support/axe";
import { createFakeDaemon, defaultFixtures, fixtureFor } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// 回答済みの決定を画面から直す（2026-10-09 人の指摘。POST /decisions/{id}/revise の UI）。
// 決定の一覧・回答イベント・revise は page.route で返す（外部ネットワークに出ない）。
test.describe.configure({ mode: "default" });
test.use({ bypassCSP: true });

const dir = makeTmpDir("celeris-work-decision-revise-");
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);
const detail = fixtureFor(schema.$defs.TaskDetail) as { task: Record<string, unknown> };
detail.task = { ...detail.task, id: "T1", title: "決定を出した親", objective: "決定の答えを直す", status: "running" };
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: { ...defaultFixtures, "/api/v1/tasks/T1": detail },
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

const options = [
  { key: "explicit", label: "(i) explicit-account" },
  { key: "implicit", label: "(ii) implicit-account" },
];
function view(id: string, over: Record<string, unknown> = {}, answer: Record<string, unknown> | null = null) {
  return {
    task_id: "T1",
    root_id: "T1",
    created_at: "2026-10-09T00:00:00Z",
    answered_at: answer ? (answer.option === "implicit" ? "2026-10-09T03:00:00Z" : "2026-10-09T01:00:00Z") : null,
    effect: answer ? "resume" : null,
    decision: {
      id,
      key: `key-${id}`,
      kind: "choice",
      question: `質問 ${id}`,
      options,
      recommended: "implicit",
      cost_of_reversal: "low",
      needed_before: [],
      path: [],
      raised_by: { origin: "planner", task_id: "T1" },
      status: answer ? "answered" : "open",
      answer,
      ...over,
    },
  };
}
const answered = (event: Record<string, unknown>, at: string, seq: number) => ({
  task_id: "T1",
  id: seq,
  ts: at,
  seq,
  event: { type: "decision_answered", ...event },
});

type Revise = { id: string; body: unknown };

async function routeDecisions(page: Page) {
  const state = {
    d1: { option: "explicit", by: "human", note: "最初の答え" } as Record<string, unknown>,
    history: [answered({ id: "D1", option: "explicit", by: "human", note: "最初の答え" }, "2026-10-09T01:00:00Z", 1)],
    revises: [] as Revise[],
  };
  await page.route("**/api/tasks/T1/decisions", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: [
          view("D1", { question: "プール外の上限をどう記録するか" }, state.d1),
          view("D2", { question: "取り下げた決定", status: "withdrawn", withdrawn_reason: "不要になった" }),
          view("D3", { question: "終わった枝の決定" }, { option: "explicit", by: "cos" }),
          view("D4", { question: "上限の決定", kind: "limit" }, { option: "explicit", by: "human" }),
        ],
      }),
    }),
  );
  await page.route("**/api/tasks/T1/events?types=decision_answered&**", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: state.history, has_more: false }),
    }),
  );
  await page.route("**/api/decisions/*/revise", async (route) => {
    const id = route.request().url().split("/").at(-2) ?? "";
    const body = route.request().postDataJSON() as { option?: string; note?: string };
    state.revises.push({ id, body });
    if (id === "D3")
      return route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({ code: "decision_not_open", detail: "node is terminal", decision_status: "answered" }),
      });
    state.d1 = { option: body.option, by: "human", note: body.note ?? null };
    state.history = [
      ...state.history,
      answered({ id: "D1", option: body.option, by: "human", note: body.note }, "2026-10-09T03:00:00Z", 2),
    ];
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        decision: view("D1", {}, state.d1),
        effect: "resume",
        resumed: [],
        cancelled: [],
        replan_requested: false,
        notified_children: ["C1"],
      }),
    });
  });
  return state;
}

test("決定: 回答済みの choice を確認付きで revise し、結果と履歴を出す（キャンセルでは送らない）", async ({ page }) => {
  const state = await routeDecisions(page);
  await page.goto(`${gateway.base}/tasks/T1#decision-D1`);
  const item = page.locator('[data-decision-id="D1"]');
  const detail = item.getByTestId("decision-detail");
  await expect(detail.getByTestId("decision-current-answer")).toContainText("(i) explicit-account");
  await expect(detail.getByTestId("decision-history").locator("li")).toHaveCount(1);
  const submit = detail.getByRole("button", { name: "答えを変える…" });
  await expect(submit).toBeDisabled();

  await detail.getByRole("radio", { name: "(ii) implicit-account" }).check();
  await detail.getByLabel("答えを変える理由").fill("人の指摘で implicit に");
  await submit.click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("「(i) explicit-account」から「(ii) implicit-account」に変えます");
  await dialog.getByRole("button", { name: "戻る（変えない）" }).click();
  await expect(dialog).toHaveCount(0);
  expect(state.revises).toEqual([]);

  await submit.click();
  await page.getByRole("alertdialog").getByRole("button", { name: "答えを「(ii) implicit-account」に変える" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(state.revises).toEqual([{ id: "D1", body: { option: "implicit", note: "人の指摘で implicit に" } }]);
  const result = item.getByTestId("decision-revise-result");
  await expect(result).toContainText("答えを「(ii) implicit-account」に変えました");
  await expect(result).toContainText("子タスク 1 件にコメントで新しい答えを届けました");
  await expect(result.getByRole("link", { name: "子タスク C1" })).toHaveAttribute("href", /\/tasks\/C1$/);
  // 一覧と履歴を読み直し、最後の回答が有効になる。
  await expect(item.getByTestId("decision-current-answer")).toContainText("(ii) implicit-account");
  const history = item.getByTestId("decision-history").locator("li");
  await expect(history).toHaveCount(2);
  await expect(history.nth(0)).toContainText("置き換え済み");
  await expect(history.nth(1)).toContainText("有効");
});

test("決定: revise できない決定は理由を出し、409 は分かる文で出す", async ({ page }) => {
  const state = await routeDecisions(page);
  await page.goto(`${gateway.base}/tasks/T1#decision-D2`);
  const withdrawn = page.locator('[data-decision-id="D2"]');
  await expect(withdrawn.getByTestId("decision-revise-unavailable")).toContainText("取り下げ済み");
  await expect(withdrawn.getByRole("button", { name: "答えを変える…" })).toHaveCount(0);

  const daemonDecision = page.locator('[data-decision-id="D4"]');
  await daemonDecision.locator("summary").click();
  await expect(daemonDecision.getByTestId("decision-revise-unavailable")).toContainText("daemon の決定");
  await expect(daemonDecision.getByTestId("decision-revise-form")).toHaveCount(0);

  const d3 = page.locator('[data-decision-id="D3"]');
  await d3.locator("summary").click();
  await d3.getByRole("radio", { name: "(ii) implicit-account" }).check();
  await d3.getByRole("button", { name: "答えを変える…" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: /に変える$/ })
    .click();
  await expect(d3.getByTestId("decision-revise-error")).toContainText("今は答えを変えられません（状態: 回答済み）");
  await expect(page.getByRole("alertdialog").getByRole("alert")).toContainText("今は答えを変えられません");
  expect(state.revises.map((r) => r.id)).toEqual(["D3"]);
});

test("決定: 4 幅で確認・変更でき、操作領域・axe・横溢れを確認する", async ({ page }) => {
  await routeDecisions(page);
  await page.goto(`${gateway.base}/tasks/T1#decision-D1`);
  const item = page.locator('[data-decision-id="D1"]');
  const form = item.getByTestId("decision-revise-form");
  const shots = process.env.DECISION_SHOT_DIR;
  if (shots) mkdirSync(shots, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await expect(form).toBeVisible();
    if (width < 768) await expect(page.locator('[data-section="decision"]')).toHaveAttribute("aria-pressed", "true");
    expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBe(0);
    for (const control of await form.locator("label, button").all()) {
      const box = await control.boundingBox();
      expect(box?.height).toBeGreaterThanOrEqual(44);
      expect(box?.width).toBeGreaterThanOrEqual(44);
    }
    expect(await seriousViolations(page)).toEqual([]);
    if (shots) await page.screenshot({ path: `${shots}/decision-before-${width}.png`, fullPage: true });
    await form.getByRole("radio", { name: "(ii) implicit-account" }).check();
    await form.getByLabel("答えを変える理由").fill("変更理由を確認");
    await form.getByRole("button", { name: "答えを変える…" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("変更理由を確認");
    await expect(dialog.getByRole("button", { name: "戻る（変えない）" })).toBeFocused();
    expect(await seriousViolations(page)).toEqual([]);
    if (shots) await page.screenshot({ path: `${shots}/decision-confirm-${width}.png`, fullPage: true });
    await dialog.getByRole("button", { name: "戻る（変えない）" }).click();
    await expect(form.getByRole("button", { name: "答えを変える…" })).toBeFocused();
  }
  await form.getByRole("button", { name: "答えを変える…" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: /に変える$/ })
    .click();
  await expect(item.getByTestId("decision-history").locator("li")).toHaveCount(2);
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBe(0);
    if (shots) await page.screenshot({ path: `${shots}/decision-after-${width}.png`, fullPage: true });
  }
});

test("決定: 履歴の取得失敗を隠さず、再取得できる", async ({ page }) => {
  await routeDecisions(page);
  await page.route("**/api/tasks/T1/events?types=decision_answered&**", (route) =>
    route.fulfill({ status: 403, body: "{}" }),
  );
  await page.goto(`${gateway.base}/tasks/T1#decision-D1`);
  const item = page.locator('[data-decision-id="D1"]');
  await expect(item.getByRole("alert")).toContainText("回答の履歴を更新できませんでした");
  await expect(item.getByTestId("decision-current-answer")).toContainText("explicit-account");
  await page.unroute("**/api/tasks/T1/events?types=decision_answered&**");
  await page.route("**/api/tasks/T1/events?types=decision_answered&**", (route) =>
    route.fulfill({ json: { items: [], has_more: false } }),
  );
  await item.getByRole("button", { name: "履歴を再取得" }).click();
  await expect(item.getByRole("alert")).toHaveCount(0);
});

test("決定: 受信箱の決定から詳細（task 詳細の決定の行）へ移る", async ({ page }) => {
  await routeDecisions(page);
  await page.goto(`${gateway.base}/inbox`);
  const item = page.locator('[data-inbox-item="decision:D1"]');
  const link = item.getByRole("link", { name: "決定の詳細と回答の履歴" });
  await expect(link).toHaveAttribute("href", "/tasks/T1#decision-D1");
  await link.click();
  await expect(page).toHaveURL(/\/tasks\/T1#decision-D1$/);
  await expect(page.locator('[data-decision-id="D1"]').getByTestId("decision-detail")).toBeVisible();
});
