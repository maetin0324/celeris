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
    await expect(panel.getByTestId("decision-status")).toHaveText("done");
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
      await expect(panel.getByTestId("execution-view")).toContainText("awaiting_human");
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
      await expect(panel.getByTestId("execution-view")).toContainText("executing");

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
