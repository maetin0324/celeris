import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import type {
  CronJobRun,
  CronJobView,
  ExecutionPlanOverview,
  ExecutionView,
  ExecutionWorkUnitView,
  TaskDetail,
  TaskExecutionView,
  WorkUnitCheckLog,
} from "../../api/generated/types";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { seriousViolations } from "../support/axe";
import { createFakeDaemon, defaultFixtures, fixtureFor } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

test.use({ bypassCSP: true });

const schema = JSON.parse(readFileSync(new URL("../../api/generated/schema.json", import.meta.url), "utf8"));
async function withGateway(
  fixtures: Record<string, unknown>,
  body: (base: string, daemon: ReturnType<typeof createFakeDaemon>) => Promise<void>,
) {
  const dir = makeTmpDir("web-cron-check-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, FIXTURE_TOKEN);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...fixtures } });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  try {
    await body(gateway.base, daemon);
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
}

test("定期実行の一覧、停止・再開・手動実行と履歴を daemon relay 経由で扱う", async ({ page }) => {
  const job = {
    ...(fixtureFor(schema.$defs.CronJobView) as CronJobView),
    id: "daily",
    name: "日次整理",
    enabled: true,
    schedule: "0 9 * * *",
    timezone: "UTC",
    next_fire_at: "2026-10-12T09:00:00Z",
  };
  const run = {
    ...(fixtureFor(schema.$defs.CronJobRun) as CronJobRun),
    id: "r1",
    task_id: "T1",
    outcome: "created",
    trigger: "manual",
  };
  await withGateway(
    {
      "/api/v1/cron-jobs": () => ({ items: [job] }),
      "/api/v1/cron-jobs/daily": () => job,
      "/api/v1/cron-jobs/daily/runs": { job_id: "daily", items: [run] },
      "/api/v1/cron-jobs/daily/pause": () => {
        job.enabled = false;
        return job;
      },
      "/api/v1/cron-jobs/daily/resume": () => {
        job.enabled = true;
        return job;
      },
      "/api/v1/cron-jobs/daily/run": { task_id: "T1", runs: [run] },
    },
    async (base, daemon) => {
      await page.goto(`${base}/cron`);
      await expect(page.getByRole("heading", { name: "定期実行", exact: true })).toBeVisible();
      await expect(page.getByText("0 9 * * * · UTC")).toBeVisible();
      await page.getByRole("button", { name: "一時停止", exact: true }).click();
      await expect(page.getByRole("button", { name: "再開", exact: true })).toBeEnabled();
      await page.getByRole("button", { name: "再開", exact: true }).click();
      await expect(page.getByRole("button", { name: "一時停止", exact: true })).toBeEnabled();
      await page.getByRole("button", { name: "手動実行", exact: true }).click();
      await expect(page.getByRole("status").filter({ hasText: "操作が完了" })).toBeVisible();
      expect(daemon.requests.filter((r) => r.method === "POST").map((r) => r.path)).toEqual([
        "/api/v1/cron-jobs/daily/pause",
        "/api/v1/cron-jobs/daily/resume",
        "/api/v1/cron-jobs/daily/run",
      ]);
      await page.getByRole("link", { name: "日次整理", exact: true }).click();
      await expect(page.getByRole("heading", { name: "実行履歴" })).toBeVisible();
      await expect(page.getByRole("link", { name: "作成したタスクを開く" })).toHaveAttribute("href", "/tasks/T1");
      await page.reload();
      await expect(page.getByRole("heading", { name: "実行履歴" })).toBeVisible();
      await page.route("**/api/cron-jobs/daily/pause", (route) =>
        route.fulfill({ status: 403, json: { detail: "禁止" } }),
      );
      await page.getByRole("button", { name: "一時停止", exact: true }).click();
      await expect(page.getByText("この操作を行う権限がありません。")).toBeVisible();
      await expect(page.getByRole("button", { name: "手動実行", exact: true })).toBeDisabled();
    },
  );
});

test("定期実行の空・取得失敗・権限不足を区別する", async ({ page }) => {
  await withGateway({ "/api/v1/cron-jobs": { items: [] } }, async (base) => {
    await page.goto(`${base}/cron`);
    await expect(page.getByText("定期実行はありません。")).toBeVisible();
    await page.route("**/api/cron-jobs", (route) => route.fulfill({ status: 500, json: {} }));
    await page.reload();
    await expect(page.getByText("取得に失敗しました。")).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText("定期実行はありません。")).toHaveCount(0);
    await page.unroute("**/api/cron-jobs");
    await page.route("**/api/cron-jobs", (route) => route.fulfill({ status: 403, json: {} }));
    await page.reload();
    await expect(page.getByText(/表示する権限がありません/)).toBeVisible();
  });
});

test("WorkUnit 検査ログは展開時だけ読み、実行中は末尾・次の検査・完了を更新する", async ({ page }) => {
  const detail = fixtureFor(schema.$defs.TaskDetail) as TaskDetail;
  detail.task = { ...detail.task, id: "T1", title: "検査中の仕事", status: "running" };
  const unit: ExecutionWorkUnitView = {
    ...(fixtureFor(schema.$defs.ExecutionWorkUnitView) as ExecutionWorkUnitView),
    id: "wu1",
    title: "統合の検査",
    status: "running",
    check_progress: { total: 2, finished: [], current: { index: 0, cmd: "first", started_at: "2026-10-11T00:00:00Z" } },
  };
  detail.execution = {
    ...(fixtureFor(schema.$defs.ExecutionView) as ExecutionView),
    plan: { ...(fixtureFor(schema.$defs.ExecutionPlanOverview) as ExecutionPlanOverview), work_units: [unit] },
  };
  const log: WorkUnitCheckLog = {
    ...(fixtureFor(schema.$defs.WorkUnitCheckLog) as WorkUnitCheckLog),
    work_unit_id: "wu1",
    index: 0,
    total: 2,
    cmd: "first",
    running: true,
    tail: "first output",
    truncated: true,
    size: 20000,
  };
  const logPath = "/api/v1/tasks/T1/work-units/wu1/check-log";
  await withGateway(
    {
      "/api/v1/tasks/T1": () => detail,
      "/api/v1/tasks/T1/execution": fixtureFor(schema.$defs.TaskExecutionView) as TaskExecutionView,
      [logPath]: () => log,
    },
    async (base, daemon) => {
      await page.goto(`${base}/tasks/T1#execution-panel`);
      const panel = page.getByTestId("work-unit-check-log");
      await expect(panel).toBeVisible();
      expect(daemon.requests.filter((r) => r.path === logPath)).toHaveLength(0);
      await panel.locator("summary").click();
      await expect(panel.getByTestId("work-unit-check-log-tail")).toHaveText("first output");
      await panel.locator("summary").click();
      const closedCount = daemon.requests.filter((r) => r.path === logPath).length;
      await page.clock.install();
      await page.clock.fastForward(15_000);
      expect(daemon.requests.filter((r) => r.path === logPath)).toHaveLength(closedCount);
      await panel.locator("summary").click();
      await expect.poll(() => daemon.requests.filter((r) => r.path === logPath).length).toBeGreaterThan(closedCount);
      log.index = 1;
      log.cmd = "second";
      log.tail = "second output";
      await expect(panel.getByTestId("work-unit-check-log-tail")).toHaveText("second output", { timeout: 12_000 });
      await expect(panel).toContainText("検査 2/2");
      log.running = false;
      log.exit = 0;
      log.tail = "finished";
      unit.check_progress = {
        total: 2,
        current: null,
        finished: [{ index: 1, cmd: "second", exit: 0, pass: true, timed_out: false, duration_ms: 10 }],
      };
      await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
      const checkEvent = (id: number) =>
        daemon.sendEvent("task.event", {
          id,
          seq: id,
          task_id: "T1",
          ts: "2026-10-11T00:00:00Z",
          event: { type: "work_unit_check_finished" },
        });
      checkEvent(1);
      await expect(panel.getByTestId("work-unit-check-log-tail")).toHaveText("finished", { timeout: 12_000 });
      await expect(panel).toContainText("exit 0");
      await expect(panel.locator("summary")).toContainText("最後の検査");
      for (const width of [360, 390, 412, 1440]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(panel.getByTestId("work-unit-check-log-tail")).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(
          0,
        );
        if (process.env.WEB_CHECK_LOG_SCREENSHOTS) {
          mkdirSync(process.env.WEB_CHECK_LOG_SCREENSHOTS, { recursive: true });
          await page.screenshot({
            path: path.join(process.env.WEB_CHECK_LOG_SCREENSHOTS, `check-log-${width}.png`),
            fullPage: true,
          });
        }
      }
      expect(await seriousViolations(page)).toEqual([]);
      await page.route("**/api/tasks/T1/work-units/wu1/check-log", (route) => route.fulfill({ status: 500, json: {} }));
      checkEvent(2);
      await expect(panel.getByText(/更新を確認できません/)).toBeVisible({ timeout: 15_000 });
      await expect(panel.getByTestId("work-unit-check-log-tail")).toHaveText("finished");
      await panel.locator("summary").click();
    },
  );
});
