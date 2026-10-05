import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures, fixtureFor } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);

const row = (id: number, taskId: string, event: Record<string, unknown>) => ({
  id,
  seq: id,
  task_id: taskId,
  ts: "2026-09-30T00:00:00Z",
  event,
});

function detail() {
  const value = fixtureFor(schema.$defs.TaskDetail) as { task: Record<string, unknown> };
  value.task = { ...value.task, id: "T1", title: "詳細の題", objective: "詳細の目的", status: "ready" };
  return value;
}

test("parity: /tasks/:id 表示", async ({ page }) => {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-task-detail-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({
    token: FIXTURE_TOKEN,
    fixtures: {
      ...defaultFixtures,
      "/api/v1/tasks/T1": detail(),
      "/api/v1/tasks/T1/timeline": { task_id: "T1", items: [] },
    },
  });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  const count = (route: string) => daemon.requests.filter((request) => request.path === route).length;
  try {
    await page.goto(`${gateway.base}/tasks/T1`);
    await expect(page.getByRole("heading", { level: 1, name: "タスクの詳細 T1" })).toBeVisible();
    await expect(page.getByTestId("task-overview")).toContainText("詳細の目的");
    await expect(page.locator("[data-tab='overview']")).toHaveAttribute("aria-current", "page");
    await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);

    // その task のイベントは detail を取り直す。他の task のイベントでは取り直さない。
    const detailBefore = count("/api/v1/tasks/T1");
    daemon.sendEvent("task.event", row(1, "other", { type: "transitioned", from: "ready", to: "running" }));
    await page.waitForTimeout(1500);
    expect(count("/api/v1/tasks/T1")).toBe(detailBefore);
    daemon.sendEvent("task.event", row(2, "T1", { type: "transitioned", from: "ready", to: "running" }));
    await expect.poll(() => count("/api/v1/tasks/T1")).toBeGreaterThan(detailBefore);

    // tab は search param。切り替えても見出しと tab は取得を待たずに出る。
    await page.locator("[data-tab='timeline']").click();
    await expect(page).toHaveURL(/tab=timeline/);
    await expect(page.locator("[data-tab='timeline']")).toHaveAttribute("aria-current", "page");
    await expect(page.getByTestId("timeline-empty")).toBeVisible();
    const timelineBefore = count("/api/v1/tasks/T1/timeline");
    daemon.sendEvent("task.event", row(3, "other", { type: "worker_progress" }));
    await page.waitForTimeout(1500);
    expect(count("/api/v1/tasks/T1/timeline")).toBe(timelineBefore);
    daemon.sendEvent("task.event", row(4, "T1", { type: "worker_progress" }));
    await expect.poll(() => count("/api/v1/tasks/T1/timeline")).toBeGreaterThan(timelineBefore);
    expect(count("/api/v1/tasks")).toBe(0);

    await page.goBack();
    await expect(page.getByTestId("task-overview")).toBeVisible();
    await page.goto(`${gateway.base}/tasks/T1?tab=timeline`);
    await expect(page.getByTestId("timeline-empty")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(
      0,
    );
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

async function withDetail(
  fixtures: Record<string, unknown | ((url: URL) => unknown)>,
  body: (base: string, daemon: ReturnType<typeof createFakeDaemon>) => Promise<void>,
) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-task-detail-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({
    token: FIXTURE_TOKEN,
    fixtures: { ...defaultFixtures, "/api/v1/tasks/T1/timeline": { task_id: "T1", items: [] }, ...fixtures },
  });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  try {
    await body(gateway.base, daemon);
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
}

test("parity: /tasks/:id 判断（expected_status と 409 再取得）", async ({ page }) => {
  let status = "reviewing";
  const reviewing = () => {
    const value = detail();
    value.task = { ...value.task, status };
    (value as Record<string, unknown>).actions = status === "reviewing" ? ["approve", "reject", "cancel", "edit"] : [];
    return value;
  };
  await withDetail({ "/api/v1/tasks/T1": () => reviewing() }, async (base, daemon) => {
    const sent: { path: string; method: string; body: unknown }[] = [];
    let approveCount = 0;
    await page.route(/\/api\/tasks\/T1(\/[a-z]+)?$/, async (route) => {
      const request = route.request();
      if (request.method() === "GET") return route.fallback();
      sent.push({ path: new URL(request.url()).pathname, method: request.method(), body: request.postDataJSON() });
      if (request.url().endsWith("/approve")) {
        approveCount += 1;
        if (approveCount === 1) {
          // 別の画面が先に動かした。409 のあとは detail を取り直して新しい状態を出す。
          status = "done";
          return route.fulfill({ status: 409, contentType: "application/json", body: '{"error":"conflict"}' });
        }
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
    });
    await page.goto(`${base}/tasks/T1`);
    const panel = page.getByTestId("decision-panel");
    await expect(panel.getByRole("heading", { name: "判断" })).toBeVisible();

    await panel.getByLabel("コメント").fill("見ました");
    await panel.getByRole("button", { name: "コメント" }).click();
    await expect(panel.getByText("操作が完了しました")).toBeVisible();
    expect(sent.at(-1)).toEqual({ path: "/api/tasks/T1/comments", method: "POST", body: { body: "見ました" } });

    await panel.getByText("編集", { exact: true }).click();
    await panel.getByLabel("題").fill("新しい題");
    await panel.getByRole("button", { name: "編集を保存" }).click();
    await expect.poll(() => sent.at(-1)?.method).toBe("PATCH");
    expect(sent.at(-1)?.body).toMatchObject({ expected_status: "reviewing", title: "新しい題" });

    const before = daemon.requests.filter((request) => request.path === "/api/v1/tasks/T1").length;
    await panel.getByLabel("理由・note（任意）").fill("確認済み");
    await panel.getByRole("button", { name: "承認" }).click();
    await expect(panel.getByText("状態が変わりました。最新の状態を確認してください。")).toBeVisible();
    expect(sent.at(-1)).toEqual({
      path: "/api/tasks/T1/approve",
      method: "POST",
      body: { expected_status: "reviewing", note: "確認済み" },
    });
    await expect
      .poll(() => daemon.requests.filter((request) => request.path === "/api/v1/tasks/T1").length)
      .toBeGreaterThan(before);
    // ここで和名の表示そのものを確かめる（他の箇所は data-status／data-phase 属性で raw 値を確かめる）。
    await expect(panel.getByTestId("decision-status")).toHaveText("完了");
    await expect(panel.getByRole("button", { name: "承認" })).toHaveCount(0);
    expect(approveCount).toBe(1);
  });
});

test("parity: /tasks/:id 実行（promote・phase_gate は確定まで成功と出さない、execution の再取得）", async ({
  page,
}) => {
  let gated = true;
  const running = () => {
    const value = detail();
    value.task = { ...value.task, status: "blocked" };
    (value as Record<string, unknown>).actions = gated ? ["phase_gate", "rereview"] : ["rereview"];
    return value;
  };
  const executionFixture = fixtureFor(schema.$defs.TaskExecutionView) as Record<string, unknown>;
  await withDetail(
    {
      "/api/v1/tasks/T1": () => running(),
      "/api/v1/tasks/T1/execution": () => ({ ...executionFixture, phase: gated ? "awaiting_human" : "executing" }),
      "/api/v1/tasks/T1/routing": {
        task_id: "T1",
        assignee: "cluster-hpc",
        runs: [{ run_id: "R1", task_id: "T1", lane: "standard", model: "m-1", org_node: "cluster-hpc" }],
      },
    },
    async (base, daemon) => {
      const sent: { path: string; body: unknown }[] = [];
      let release: () => void = () => {};
      await page.route(/\/api\/tasks\/T1\/(rereview|execution\/[a-z-]+|artifacts\/promote)$/, async (route) => {
        const request = route.request();
        if (request.method() === "GET") return route.fallback();
        const pathname = new URL(request.url()).pathname;
        sent.push({ path: pathname, body: request.postDataJSON() });
        if (pathname.endsWith("/phase-gate") || pathname.endsWith("/promote")) {
          // 応答を止めている間は成功と出さない。
          await new Promise<void>((resolve) => {
            release = resolve;
          });
          gated = false;
        }
        return route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
      });
      const count = (route: string) => daemon.requests.filter((request) => request.path === route).length;
      await page.goto(`${base}/tasks/T1`);
      const panel = page.getByTestId("execution-panel");
      await expect(panel.getByTestId("execution-view").locator("[data-phase='awaiting_human']")).toBeVisible();
      await expect(panel.getByTestId("routing-panel")).toContainText("m-1");

      await panel.getByLabel("途中確認の note（任意）").fill("先へ");
      await panel.getByRole("button", { name: "続ける" }).click();
      await expect(panel.getByText("確定を待っています")).toBeVisible();
      await page.waitForTimeout(300);
      await expect(panel.getByText("確定しました")).toHaveCount(0);
      release();
      await expect(panel.getByText("確定しました")).toBeVisible();
      expect(sent.at(-1)).toEqual({
        path: "/api/tasks/T1/execution/phase-gate",
        body: { action: "continue", note: "先へ" },
      });
      await expect(panel.getByTestId("execution-view").locator("[data-phase='executing']")).toBeVisible();

      await panel.getByText("成果物を文書に昇格").click();
      await panel.getByLabel("成果物の名前").fill("report.md");
      await panel.getByLabel("文書の path").fill("docs/report.md");
      await panel.getByRole("button", { name: "昇格する" }).click();
      const promote = panel.getByTestId("promote");
      await expect(promote.getByText("確定を待っています")).toBeVisible();
      await expect(promote.getByText("確定しました")).toHaveCount(0);
      release();
      await expect(promote.getByText("確定しました")).toBeVisible();
      expect(sent.at(-1)).toEqual({
        path: "/api/tasks/T1/artifacts/promote",
        body: { name: "report.md", path: "docs/report.md" },
      });

      await panel.getByRole("button", { name: "再レビュー" }).click();
      await expect.poll(() => sent.at(-1)?.path).toBe("/api/tasks/T1/rereview");
      await panel.getByRole("button", { name: "計画を作らせる" }).click();
      await expect
        .poll(() => sent.at(-1))
        .toEqual({ path: "/api/tasks/T1/execution/decompose", body: { mode: "compound" } });

      // execution のイベントは execution の key を取り直す。他の task のイベントでは取り直さない。
      const before = count("/api/v1/tasks/T1/execution");
      daemon.sendEvent("task.event", row(10, "other", { type: "execution_planned" }));
      await page.waitForTimeout(1500);
      expect(count("/api/v1/tasks/T1/execution")).toBe(before);
      daemon.sendEvent("task.event", row(11, "T1", { type: "execution_planned" }));
      await expect.poll(() => count("/api/v1/tasks/T1/execution")).toBeGreaterThan(before);
    },
  );
});

function changesFixture() {
  const repo = fixtureFor(schema.$defs.RepoChangesView) as Record<string, unknown>;
  const integration = fixtureFor(schema.$defs.TaskIntegration) as Record<string, unknown>;
  return {
    task_id: "T1",
    gh: true,
    merge_method: "squash",
    delivery: null,
    repos: [
      {
        ...repo,
        repo: "web",
        branch: "celeris/T1",
        default_branch: "main",
        ahead: 2,
        dirty: false,
        missing: false,
        files: [{ path: "src/long.ts", status: "M", additions: 3, deletions: 1 }],
        integration: { ...integration, repo: "web", method: "pr", state: "open", pr_number: 7, pr_url: null },
      },
    ],
  };
}

const longDiff = `--- a/src/long.ts\n+++ b/src/long.ts\n@@ -1 +1 @@\n-old\n+${"x".repeat(600)}\n`;

function changesFixtures() {
  const tree = fixtureFor(schema.$defs.TreeView) as Record<string, unknown>;
  return {
    "/api/v1/tasks/T1/changes": changesFixture(),
    "/api/v1/tasks/T1/changes/web/diff": { repo: "web", path: "src/long.ts", diff: longDiff, truncated: false },
    "/api/v1/tasks/T1/tree": { ...tree, entries: [{ name: "README.md", path: "README.md", kind: "file", size: 3 }] },
    "/api/v1/tasks/T1/artifacts": { task_id: "T1", items: [] },
  };
}

const overflow = (page: import("@playwright/test").Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);

test("parity: /tasks/:id 5 tab と全 intent・409", async ({ page }) => {
  const full = () => {
    const value = detail();
    value.task = { ...value.task, status: "reviewing" };
    Object.assign(value, {
      latest_question: "どちらにしますか",
      actions: ["approve", "reject", "answer", "cancel", "retry", "edit", "reopen", "rereview", "phase_gate"],
    });
    return value;
  };
  await withDetail({ "/api/v1/tasks/T1": () => full(), ...changesFixtures() }, async (base) => {
    const sent: { path: string; method: string; body: unknown }[] = [];
    await page.route(/\/api\/tasks\/T1(\/.*)?$/, async (route) => {
      const request = route.request();
      if (request.method() === "GET") return route.fallback();
      const pathname = new URL(request.url()).pathname;
      sent.push({ path: pathname, method: request.method(), body: request.postDataJSON() });
      if (pathname.endsWith("/cancel"))
        return route.fulfill({ status: 409, contentType: "application/json", body: '{"error":"conflict"}' });
      return route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
    });
    await page.goto(`${base}/tasks/T1`);
    const decision = page.getByTestId("decision-panel");
    const execution = page.getByTestId("execution-panel");
    await expect(decision.getByTestId("decision-status")).toHaveAttribute("data-status", "reviewing");
    const last = () => sent.at(-1)?.path;
    const click = async (scope: typeof decision, name: string, path: string) => {
      await scope.getByRole("button", { name, exact: true }).click();
      await expect.poll(last).toBe(path);
      await expect(scope.getByRole("button", { name, exact: true })).toBeEnabled();
    };

    // 判断（P3-09）: approve / reject / answer / retry / reopen / comment / edit、cancel は 409。
    await click(decision, "承認", "/api/tasks/T1/approve");
    await click(decision, "却下", "/api/tasks/T1/reject");
    await decision.getByLabel("回答").fill("A にします");
    await click(decision, "回答", "/api/tasks/T1/answer");
    await click(decision, "やり直す", "/api/tasks/T1/retry");
    await click(decision, "再開", "/api/tasks/T1/reopen");
    await decision.getByLabel("コメント").fill("見ました");
    await click(decision, "コメント", "/api/tasks/T1/comments");
    await decision.getByText("編集", { exact: true }).click();
    await decision.getByLabel("題").fill("新しい題");
    await click(decision, "編集を保存", "/api/tasks/T1");
    expect(sent.at(-1)?.method).toBe("PATCH");
    await decision.getByRole("button", { name: "中止", exact: true }).click();
    await expect.poll(last).toBe("/api/tasks/T1/cancel");
    await expect(decision.getByText("状態が変わりました。最新の状態を確認してください。")).toBeVisible();

    // 実行（P3-10）: phase_gate / rereview / execution_decompose / promote。
    await execution.getByRole("button", { name: "続ける" }).click();
    await expect.poll(last).toBe("/api/tasks/T1/execution/phase-gate");
    await expect(execution.getByText("確定しました")).toBeVisible();
    await click(execution, "再レビュー", "/api/tasks/T1/rereview");
    await click(execution, "計画を作らせる", "/api/tasks/T1/execution/decompose");
    await execution.getByText("成果物を文書に昇格").click();
    await execution.getByLabel("成果物の名前").fill("report.md");
    await execution.getByLabel("文書の path").fill("docs/report.md");
    await execution.getByRole("button", { name: "昇格する" }).click();
    await expect.poll(last).toBe("/api/tasks/T1/artifacts/promote");

    // 5 tab: どれも見出しを保ち、横に溢れない。
    const tabs: [string, string][] = [
      ["timeline", "timeline-empty"],
      ["changes", "task-changes"],
      ["files", "files-tree"],
      ["artifacts", "task-artifacts-empty"],
      ["overview", "task-overview"],
    ];
    for (const [tab, testId] of tabs) {
      await page.locator(`[data-tab='${tab}']`).click();
      await expect(page.locator(`[data-tab='${tab}']`)).toHaveAttribute("aria-current", "page");
      await expect(page.getByRole("heading", { level: 1, name: "タスクの詳細 T1" })).toBeVisible();
      await expect(page.getByTestId(testId)).toBeVisible();
      expect(await overflow(page)).toBe(0);
    }
    await page.goto(`${base}/tasks/T1?tab=changes`);
    await expect(page.getByTestId("task-changes")).toContainText("src/long.ts");
  });
});

test("parity: /tasks/:id/changes 差分・取り込み・merge", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await withDetail(changesFixtures(), async (base, daemon) => {
    const sent: { path: string; body: unknown }[] = [];
    await page.route(/\/api\/tasks\/T1\/changes\/web\/(integrate|pr\/merge)$/, async (route) => {
      const request = route.request();
      const pathname = new URL(request.url()).pathname;
      const body = request.postDataJSON() as Record<string, unknown>;
      sent.push({ path: pathname, body });
      if (body.method === "discard" && body.confirm !== true)
        return route.fulfill({
          status: 422,
          contentType: "application/json",
          body: JSON.stringify({ error: "validation", message: "confirm が必要です" }),
        });
      const state = pathname.endsWith("/pr/merge") ? "merged" : "conflict";
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          integration: { id: "I2", task_id: "T1", repo: "web", method: "merge", state, created_at: "", updated_at: "" },
          child_task_id: state === "conflict" ? "T9" : null,
        }),
      });
    });
    const count = () => daemon.requests.filter((request) => request.path === "/api/v1/tasks/T1/changes").length;
    await page.goto(`${base}/tasks/T1/changes`);
    await expect(page.getByRole("heading", { level: 1, name: "変更 T1" })).toBeVisible();
    await expect(page.getByTestId("integration-state")).toContainText("pr / open");

    // 差分: 長い行は枠（pre）の中で横に scroll し、画面は溢れない。
    await page.locator("[data-file='src/long.ts']").click();
    const pre = page.getByTestId("change-diff-body");
    await expect(pre).toContainText("xxxx");
    expect(await pre.evaluate((node) => node.scrollWidth > node.clientWidth)).toBe(true);
    expect(await overflow(page)).toBe(0);

    // integrate: 衝突は 200 のまま結果と「衝突の解消」タスクを出し、changes を取り直す。
    const before = count();
    await page.getByLabel("取り込みの note（任意）").fill("入れます");
    await page.getByRole("button", { name: "取り込む" }).click();
    await expect(page.getByTestId("integrate-result")).toContainText("conflict");
    await expect(page.getByTestId("integrate-result").getByRole("link", { name: "T9" })).toBeVisible();
    expect(sent.at(-1)).toEqual({
      path: "/api/tasks/T1/changes/web/integrate",
      body: { method: "merge", note: "入れます" },
    });
    await expect.poll(count).toBeGreaterThan(before);

    // discard は確認が無ければ daemon の 422 をそのまま出す。確認すれば confirm: true を送る。
    await page.getByLabel("取り込みの方法").selectOption("discard");
    await page.getByRole("button", { name: "取り込む" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "confirm が必要です" })).toBeVisible();
    await page.getByLabel("取り返しがつかないことを確認した").check();
    await page.getByRole("button", { name: "取り込む" }).click();
    await expect.poll(() => sent.at(-1)?.body).toEqual({ method: "discard", note: "入れます", confirm: true });

    // pr_merge: 開いている PR を Celeris で merge する。
    await page.getByRole("button", { name: "Celeris で merge（squash）" }).click();
    await expect.poll(() => sent.at(-1)?.path).toBe("/api/tasks/T1/changes/web/pr/merge");
    await expect(page.getByTestId("integrate-result")).toContainText("merged");

    // その task の取り込みのイベントだけが changes を取り直す。
    const scoped = count();
    daemon.sendEvent("task.event", row(20, "other", { type: "phase_integrated" }));
    await page.waitForTimeout(1500);
    expect(count()).toBe(scoped);
    daemon.sendEvent("task.event", row(21, "T1", { type: "phase_integrated" }));
    await expect.poll(count).toBeGreaterThan(scoped);
  });
});

// 観測性の header と木（2026-10-04 screens-ops）。長い ID・題を持つ fixture で、状態・現在の run・次の操作、
// 親 task・段・WU・子 task・main から取り込んだ integration repair の行を確かめ、360px で横に溢れないことを見る。
const longId = `T1-${"0123456789".repeat(6)}`;
const longTitle = `長い題の task ${"とても長い説明".repeat(20)}`;

function observedFixtures() {
  const run = fixtureFor(schema.$defs.RunSummary) as Record<string, unknown>;
  const unit = (seq: number, key: string, phase: string, status: string, extra: Record<string, unknown> = {}) => ({
    ...(fixtureFor(schema.$defs.WorkUnitView) as Record<string, unknown>),
    id: `WU-${key}-${"x".repeat(40)}`,
    key,
    seq,
    phase,
    status,
    spec: {
      ...(fixtureFor(schema.$defs.WorkUnitSpec) as Record<string, unknown>),
      key,
      title: `WU ${key} の題`,
      phase,
    },
    ...extra,
  });
  const plan = fixtureFor(schema.$defs.ExecutionPlanView) as Record<string, unknown>;
  const execution = fixtureFor(schema.$defs.TaskExecutionView) as Record<string, unknown>;
  const value = detail() as Record<string, unknown> & { task: Record<string, unknown> };
  value.task = { ...value.task, status: "running", parent_id: `P-${"9".repeat(60)}`, title: longTitle };
  Object.assign(value, {
    actions: ["approve", "reject", "retry", "phase_gate"],
    runs: [
      {
        ...run,
        run_id: "R-old",
        started_at: "2026-10-01T00:00:00Z",
        finished_at: "2026-10-01T01:00:00Z",
        outcome: "done",
      },
      { ...run, run_id: `R-${"7".repeat(60)}`, started_at: "2026-10-02T00:00:00Z", finished_at: null, end: null },
    ],
    children: [{ id: longId, title: "子の題", kind: "execute", status: "blocked", actions: [] }],
    integration_repair: {
      state: "scheduled",
      attempt: 1,
      max_attempts: 2,
      target_ref: "main",
      target_sha: "a".repeat(40),
      work_unit_id: `WU-merge-${"x".repeat(40)}`,
    },
  });
  return {
    "/api/v1/tasks/T1": value,
    "/api/v1/tasks/T1/execution": {
      ...execution,
      phase: "executing",
      plan: {
        ...plan,
        version: 2,
        plan: {
          ...(plan.plan as Record<string, unknown>),
          stages: [
            { key: "build", title: "作る", kind: "implement" },
            { key: "verify", title: "確かめる", kind: "test" },
          ],
        },
        work_units: [
          unit(1, "header-tree", "build", "running", { running_run_id: "R-wu" }),
          unit(2, "merge", "build", "done"),
          unit(3, "detail-verify", "verify", "pending"),
        ],
      },
    },
  };
}

test("parity: /tasks/:id header（状態・現在の run・次の操作）と木（親子 task・段・WU・integration repair）", async ({
  page,
}) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await withDetail(observedFixtures(), async (base) => {
    await page.goto(`${base}/tasks/T1`);
    await expect(page.getByRole("heading", { level: 1, name: "タスクの詳細 T1" })).toBeVisible();

    // header: 状態は StatusBadge、現在の run は終わっていない run、次の操作は今できる判断だけ。
    const header = page.getByTestId("task-header");
    await expect(header.getByTestId("task-header-status").locator("[data-status='running']")).toHaveText("実行中");
    const runLink = header.getByRole("link", { name: `run R-${"7".repeat(60)} を開く` });
    await expect(runLink).toHaveAttribute("href", `/tasks/T1/runs/R-${"7".repeat(60)}`);
    await expect(header.locator("[data-slot='short-id']").first()).toHaveAttribute("title", `R-${"7".repeat(60)}`);
    await expect(header.getByTestId("task-header-run").locator("[data-status='running']")).toBeVisible();
    const next = header.getByTestId("task-header-next");
    await expect(next.locator("[data-action]")).toHaveCount(3);
    await expect(next.getByRole("link", { name: "承認を待っています" })).toHaveAttribute("href", /#decision-panel$/);
    await expect(next.getByRole("link", { name: "途中確認の決定を待っています" })).toHaveAttribute(
      "href",
      /#execution-panel$/,
    );
    await expect(next.getByRole("link", { name: "再試行できます" })).toBeVisible();

    // 判断 panel の button 名は変えていない。スマホ幅では「次の操作」の link が判断の区画を開く。
    await next.getByRole("link", { name: "承認を待っています" }).click();
    await expect(page.locator("[data-section='decision']")).toHaveAttribute("aria-pressed", "true");
    const decision = page.getByTestId("decision-panel");
    for (const name of ["承認", "却下", "やり直す", "コメント"])
      await expect(decision.getByRole("button", { name, exact: true })).toBeVisible();

    // 木: 親 task → この task → 段 → WU、WU に紐づく integration repair、子 task。スマホ幅では木の区画で見る。
    await page.locator("[data-section='tree']").click();
    const tree = page.getByTestId("task-tree");
    await expect(tree.getByTestId("tree-parent").getByRole("link", { name: /^親 task P-9+ を開く$/ })).toBeVisible();
    await expect(tree.getByTestId("tree-self").first()).toContainText("実行中");
    await expect(tree.getByTestId("tree-stage")).toHaveCount(2);
    await expect(tree.locator("[data-stage='build']")).toContainText("作る");
    await expect(tree.locator("[data-stage='build'] [data-testid='tree-work-unit']")).toHaveCount(2);
    await expect(tree.locator("[data-stage='verify'] [data-testid='tree-work-unit']")).toHaveCount(1);
    const wu = tree.locator("[data-key='header-tree']");
    await expect(wu).toContainText("WU header-tree の題");
    await expect(wu.locator("[data-status='running']")).toBeVisible();
    // スマホ幅では WU の key・run は Drawer（木の詳細）で開く。
    await wu.getByRole("button", { name: "WU header-tree の詳細" }).click();
    const wuDrawer = page.getByRole("dialog", { name: "WU WU header-tree の題" });
    await expect(wuDrawer.getByRole("link", { name: "WU header-tree の run R-wu を開く" })).toBeVisible();
    await wuDrawer.getByRole("button", { name: "閉じる" }).click();
    await expect(wuDrawer).toHaveCount(0);
    const repair = tree.locator("[data-key='merge'] [data-testid='tree-integration-repair']");
    await expect(repair).toHaveAttribute("data-state", "scheduled");
    await expect(repair).toContainText("main から取り込み");
    await expect(repair).toContainText("修復中（scheduled）");
    const child = tree.getByTestId("tree-child");
    await expect(child).toContainText("子の題");
    await expect(child.locator("[data-status='blocked']")).toBeVisible();
    await expect(child.locator("[data-slot='short-id']")).toHaveAttribute("title", longId);

    // 長い ID は省略（全文は title）、長い題は折り返し。360px で横に溢れない。
    expect(await overflow(page)).toBe(0);

    // 記録用の screenshot（TASK_DETAIL_SHOT_DIR を渡したときだけ）。スマホ 3 幅とデスクトップ幅。
    const shotDir = process.env.TASK_DETAIL_SHOT_DIR;
    if (shotDir) {
      for (const width of [360, 390, 412, 1440]) {
        await page.setViewportSize({ width, height: 900 });
        expect(await overflow(page)).toBe(0);
        await page.screenshot({ path: path.join(shotDir, `task-detail-after-${width}.png`), fullPage: true });
      }
    }
  });
});

// スマホ幅（360〜412）の区画切り替えと Drawer（2026-10-04 screens-ops mobile）。長い ID・題の fixture で、
// どの区画でも横 scroll が出ないこと、長い ID は省略・長い題は折り返し/省略して title 属性で全文が読めること、
// 操作が 44×44 以上であることを見る。desktop（1440）は切り替えを出さず全区画を並べたまま。
test("parity: /tasks/:id スマホ幅の区画 tab と Drawer、長い ID・題、desktop の配置", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await withDetail(observedFixtures(), async (base) => {
    await page.goto(`${base}/tasks/T1`);
    await expect(page.getByRole("heading", { level: 1, name: "タスクの詳細 T1" })).toBeVisible();
    await expect(page.locator("[data-tab='overview']")).toHaveAttribute("aria-current", "page");
    const sections = page.getByTestId("mobile-sections");
    await expect(sections).toBeVisible();
    const tapSize = async (locator: import("@playwright/test").Locator) => {
      const box = await locator.boundingBox();
      expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
      expect(box?.width ?? 0).toBeGreaterThanOrEqual(44);
    };

    // 長い題: header は省略（全文は title 属性）、長い ID は 1 行で省略（全文は title 属性）。
    const headerTitle = page.getByTestId("task-header").locator("p[title]").first();
    await expect(headerTitle).toHaveAttribute("title", longTitle);
    expect(await headerTitle.evaluate((node) => node.scrollHeight > node.clientHeight)).toBe(true);
    const runId = page.getByTestId("task-header-run").locator("[data-slot='short-id']");
    await expect(runId).toHaveAttribute("title", `R-${"7".repeat(60)}`);
    expect(await runId.evaluate((node) => node.scrollWidth > node.clientWidth)).toBe(true);

    for (const width of [360, 390, 412]) {
      await page.setViewportSize({ width, height: 800 });
      // 上の 5 tab は 1 行で、溢れは nav の中だけで scroll する。
      const tabBox = await page.locator("[data-tab='files']").boundingBox();
      expect(tabBox?.height ?? 0).toBeLessThan(60);
      const visible: [string, string, string][] = [
        ["summary", "task-overview", "task-tree"],
        ["decision", "decision-panel", "execution-panel"],
        ["execution", "execution-panel", "decision-panel"],
        ["tree", "task-tree", "decision-panel"],
      ];
      for (const [section, shown, hidden] of visible) {
        const button = sections.locator(`[data-section='${section}']`);
        await button.click();
        await expect(button).toHaveAttribute("aria-pressed", "true");
        await tapSize(button);
        await expect(page.getByTestId(shown)).toBeVisible();
        await expect(page.getByTestId(hidden)).toBeHidden();
        expect(await overflow(page)).toBe(0);
      }
    }

    // 木の詳細と integration repair は Drawer で開く。Drawer の中でも横に溢れない。
    await page.setViewportSize({ width: 360, height: 800 });
    const tree = page.getByTestId("task-tree");
    await expect(tree.getByTestId("tree-self").locator("[title]").first()).toHaveAttribute("title", longTitle);
    await expect(page.locator("#integration-repair")).toBeHidden();
    const repairButton = tree
      .getByTestId("tree-integration-repair")
      .getByRole("button", { name: "integration repair" });
    await tapSize(repairButton);
    await repairButton.click();
    const repairDrawer = page.getByRole("dialog");
    await expect(repairDrawer.getByTestId("integration-repair")).toHaveAttribute("data-state", "scheduled");
    await expect(repairDrawer).toContainText("a".repeat(40));
    expect(await overflow(page)).toBe(0);
    await page.keyboard.press("Escape");
    await expect(repairDrawer).toHaveCount(0);
    await expect(repairButton).toBeFocused();

    const detailButton = tree.locator("[data-key='merge']").getByRole("button", { name: "WU merge の詳細" });
    await tapSize(detailButton);
    await detailButton.click();
    const wuDrawer = page.getByRole("dialog", { name: "WU WU merge の題" });
    await expect(wuDrawer).toContainText(`WU-merge-${"x".repeat(40)}`);
    expect(await overflow(page)).toBe(0);
    await page.keyboard.press("Escape");

    // header の「途中確認」の link は実行の区画を開く。
    await page.getByRole("link", { name: "途中確認の決定を待っています" }).click();
    await expect(sections.locator("[data-section='execution']")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("execution-panel")).toBeVisible();

    // desktop（1440）: 区画の切り替えは出さず、判断・実行・概要・木・integration repair を並べる。WU の run は行に出る。
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(sections).toBeHidden();
    for (const id of ["decision-panel", "execution-panel", "task-overview", "task-tree"])
      await expect(page.getByTestId(id)).toBeVisible();
    await expect(page.locator("#integration-repair")).toBeVisible();
    await expect(
      tree.locator("[data-key='header-tree']").getByRole("link", { name: "WU header-tree の run R-wu を開く" }),
    ).toBeVisible();
    await expect(tree.getByRole("button", { name: "WU header-tree の詳細" })).toBeHidden();
    expect(await overflow(page)).toBe(0);

    // 記録用の screenshot（TASK_DETAIL_SHOT_DIR を渡したときだけ）。スマホ 3 幅は区画ごと、desktop は全体。
    const shotDir = process.env.TASK_DETAIL_SHOT_DIR;
    if (shotDir) {
      for (const width of [360, 390, 412]) {
        await page.setViewportSize({ width, height: 900 });
        for (const section of ["summary", "decision", "tree"]) {
          await sections.locator(`[data-section='${section}']`).click();
          await page.screenshot({
            path: path.join(shotDir, `task-detail-mobile-${section}-${width}.png`),
            fullPage: true,
          });
        }
      }
      await tree.locator("[data-key='merge']").getByRole("button", { name: "WU merge の詳細" }).click();
      await page.screenshot({ path: path.join(shotDir, "task-detail-mobile-drawer-412.png") });
      await page.keyboard.press("Escape");
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.screenshot({ path: path.join(shotDir, "task-detail-mobile-after-1440.png"), fullPage: true });
    }
  });
});
