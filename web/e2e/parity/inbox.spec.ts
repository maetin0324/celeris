import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

const dir = mkdtempSync(path.join(tmpdir(), "celeris-inbox-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const baseInbox = defaultFixtures["/api/v1/inbox"] as { counts: Record<string, number>; [key: string]: unknown };
const task = (id: string, status = "reviewing") => ({
  id,
  title: `確認 ${id}`,
  status,
  kind: "task",
  actions: ["approve", "reject", "answer", "cancel"],
});
const inbox = {
  ...baseInbox,
  approvals: [
    {
      approval: task("T1"),
      artifacts: [{ idx: 0, name: "report.md", kind: "report", path: "report.md", sha256: "x" }],
      criterion_text: "**確認**してください",
      evidence: [],
      knowledge_pages: [],
      other_verdicts: [],
      previous_decisions: [],
      requested_at: "2026-09-30T00:00:00Z",
    },
  ],
  questions: [{ task: task("T2", "blocked"), question: "回答してください", previous: [] }],
  drafts: [],
  attention: [{ type: "cluster_unavailable", cluster: "sirius", host: "node", tasks: 1, at: "2026-09-30T00:00:00Z" }],
  counts: { ...baseInbox.counts, approvals: 1, questions: 1, drafts: 0, attention: 1 },
};
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: { ...defaultFixtures, "/api/v1/inbox": inbox },
  files: { "/api/v1/tasks/T1/artifacts/0": { body: "# Report\n\nbody", type: "text/markdown" } },
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

test("parity: /inbox 区画表示と承認・409 再取得", async ({ page }) => {
  let sent: unknown;
  await page.route("**/api/tasks/T1/approve", async (route) => {
    sent = route.request().postDataJSON();
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  await page.goto(`${gateway.base}/inbox`);
  await expect(page.getByRole("heading", { name: "承認待ち（1）" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "質問（1）" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "注意（1）" })).toBeVisible();
  await page.getByRole("button", { name: "本文をここで見る" }).click();
  await expect(page.getByRole("heading", { name: "Report" })).toBeVisible();
  await page.getByLabel("理由・note（任意）").first().fill("確認済み");
  await page.getByRole("listitem").filter({ hasText: "確認 T1" }).getByRole("button", { name: "承認" }).click();
  await expect(page.getByText("操作が完了しました").first()).toBeVisible();
  expect(sent).toEqual({ expected_status: "reviewing", note: "確認済み" });
});

test("parity-x: 409 再取得・422 表示・二重送信なし", async ({ page }) => {
  let count = 0;
  await page.route("**/api/tasks/T2/answer", async (route) => {
    count += 1;
    await new Promise((resolve) => setTimeout(resolve, 200));
    await route.fulfill({
      status: count === 1 ? 422 : 409,
      contentType: "application/json",
      body: JSON.stringify({ detail: count === 1 ? "回答が不正です" : "conflict" }),
    });
  });
  await page.goto(`${gateway.base}/inbox`);
  const question = page.getByRole("listitem").filter({ hasText: "確認 T2" });
  await question.getByLabel("回答").fill("再入力を残す");
  await question.getByRole("button", { name: "回答" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await expect(question.getByRole("alert")).toHaveText("回答が不正です");
  expect(count).toBe(1);
  await expect(question.getByLabel("回答")).toHaveValue("再入力を残す");
  const before = daemon.requests.filter((request) => request.path === "/api/v1/inbox").length;
  await question.getByRole("button", { name: "回答" }).click();
  await expect(question.getByRole("alert")).toContainText("状態が変わりました");
  await expect
    .poll(() => daemon.requests.filter((request) => request.path === "/api/v1/inbox").length)
    .toBeGreaterThan(before);
});

// S7 の補助: 偽 daemon の実データを表示した状態を 4 幅で保存する。
test("/inbox fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  mkdirSync(out as string, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${gateway.base}/inbox`);
    await expect(page.getByRole("heading", { name: "承認待ち（1）" })).toBeVisible();
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
  const requests: { path: string; method: string; body?: string }[] = [];
  await page.route("**/api/approvals?**", async (route) => {
    const pending = new URL(route.request().url()).searchParams.get("pending") === "true";
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: pending ? [approvalFixture] : [] }),
    });
  });
  await page.route("**/api/approvals/A1/decide", async (route) => {
    requests.push({ path: "decide", method: route.request().method(), body: route.request().postData() ?? undefined });
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  await page.route("**/api/standing-rules**", async (route) => {
    requests.push({ path: "rules", method: route.request().method(), body: route.request().postData() ?? undefined });
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [{ id: "S1", rule: "既定の規則", created_at: "2026-09-30T00:00:00Z" }] }),
    });
  });
  await page.route("**/api/daemon", async (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ...(defaultFixtures["/api/v1/daemon"] as object), approvals_pending: 3 }),
    }),
  );
  await page.goto(`${gateway.base}/approvals`);
  await expect(page.getByRole("heading", { name: "認可待ち" })).toBeVisible();
  await expect(page.getByLabel("承認待ち 3 件")).toBeVisible();
  await page.getByLabel("回答").fill("許可します");
  await page.getByRole("button", { name: "今回だけ認める" }).click();
  await expect(page.getByText("操作が完了しました").first()).toBeVisible();
  expect(JSON.parse(requests.find((request) => request.path === "decide")?.body ?? "null")).toEqual({
    answer: "許可します",
    decision: "once",
  });
  await page.getByLabel("規則文").fill("新しい規則");
  await page.getByRole("button", { name: "追加" }).click();
  await expect
    .poll(() => requests.filter((request) => request.path === "rules" && request.method === "POST").length)
    .toBe(1);
  await page.getByRole("button", { name: "削除" }).click();
  await expect
    .poll(() => requests.filter((request) => request.path === "rules" && request.method === "DELETE").length)
    .toBe(1);
  daemon.sendEvent("daemon", { snapshot: { approvals_pending: 0 } });
  await expect(page.getByLabel("承認待ち 3 件")).toBeVisible();
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
