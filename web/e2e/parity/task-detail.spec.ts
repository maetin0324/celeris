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
