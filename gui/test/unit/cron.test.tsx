import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { applyCronAction, loadCronJobDetail, loadCronJobs } from "~/celeris/cron.server";
import type { CronJobRun, CronJobView } from "~/celeris/types";
import CronPage from "~/routes/cron";
import CronDetailPage from "~/routes/cron.$id";
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

const run: CronJobRun = {
  id: "run-1",
  job_id: "job-1",
  scheduled_for: "2026-10-03T00:00:00Z",
  recorded_at: "2026-10-03T00:00:01Z",
  trigger: "schedule",
  outcome: "created",
  task_id: "task-1",
};
const job: CronJobView = {
  id: "job-1",
  name: "daily-curation",
  enabled: true,
  schedule: "0 0 * * *",
  timezone: "Asia/Tokyo",
  overlap: "skip",
  catch_up: "latest",
  next_fire_at: "2026-10-04T00:00:00Z",
  created_at: "2026-10-02T00:00:00Z",
  updated_at: "2026-10-02T00:00:00Z",
  template: { title: "整理 {date}" },
  last_run: run,
};

let mock: MockCeleris;
let client: CelerisClient;
beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});
afterEach(async () => {
  await mock.close();
});

describe("cron BFF", () => {
  it("loads the list and detail with history from the API", async () => {
    mock.on("GET", "/api/v1/cron-jobs", (_req, res) => sendJson(res, 200, { items: [job] }));
    mock.on("GET", "/api/v1/cron-jobs/job-1", (_req, res) => sendJson(res, 200, job));
    mock.on("GET", "/api/v1/cron-jobs/job-1/runs", (_req, res) => sendJson(res, 200, { job_id: job.id, items: [run] }));
    expect(await loadCronJobs(client)).toEqual({ items: [job] });
    expect(await loadCronJobDetail(client, job.id)).toEqual({ job, history: { job_id: job.id, items: [run] } });
  });

  it("sends pause, resume and manual run to the selected job", async () => {
    for (const intent of ["pause", "resume", "run"] as const) {
      mock.on("POST", `/api/v1/cron-jobs/job-1/${intent}`, (_req, res) =>
        sendJson(
          res,
          200,
          intent === "run" ? { job_id: job.id, job_name: job.name, runs: [run], task_id: run.task_id } : job,
        ),
      );
      const result = await applyCronAction(client, job.id, intent);
      expect(result.ok).toBe(true);
      expect(mock.requests.at(-1)).toMatchObject({
        method: "POST",
        url: `/api/v1/cron-jobs/job-1/${intent}`,
        body: "{}",
      });
    }
  });

  it("returns API conflicts for display instead of claiming a run was created", async () => {
    mock.on("POST", "/api/v1/cron-jobs/job-1/run", (_req, res) =>
      sendProblem(res, { status: 409, code: "cron_run_conflict", detail: "already running" }),
    );
    const result = await applyCronAction(client, job.id, "run");
    expect(result).toMatchObject({
      ok: false,
      error: { status: 409, code: "cron_run_conflict", detail: "already running" },
    });
  });
});

function render(path: string, element: ReactElement): string {
  const router = createMemoryRouter([{ path, element }], { initialEntries: [path] });
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

describe("cron pages", () => {
  it("shows schedule, timezone, next fire, previous result and controls", () => {
    const html = render("/cron", createElement(CronPage, { loaderData: { items: [job] } } as never));
    expect(html).toContain("daily-curation");
    expect(html).toContain("0 0 * * *");
    expect(html).toContain("Asia/Tokyo");
    expect(html).toContain("2026-10-04T00:00:00Z");
    expect(html).toContain("created");
    expect(html).toContain("一時停止");
    expect(html).toContain("手動実行");
  });

  it("shows run history and links to the created task", () => {
    const html = render(
      "/cron/:id",
      createElement(CronDetailPage, { loaderData: { job, history: { job_id: job.id, items: [run] } } } as never),
    );
    expect(html).toContain("実行履歴");
    expect(html).toContain("schedule");
    expect(html).toContain("/tasks/task-1");
    expect(html).toContain("整理 {date}");
  });
});
