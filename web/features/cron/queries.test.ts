import { afterEach, expect, it, vi } from "vitest";
import { configureApiClient } from "../../api/client";
import { checkLogQuery } from "../tasks/work-unit-check-log";
import { cronDetailQuery, cronPath } from "./queries";

afterEach(() => {
  vi.restoreAllMocks();
  configureApiClient({ fetcher: (i, init) => fetch(i, init) });
});
it("encodes job and task identifiers as single path segments", async () => {
  expect(cronPath("job /?#")).toBe("/api/cron-jobs/job%20%2F%3F%23");
  const fetcher = vi.fn<typeof fetch>(async () => new Response("{}"));
  configureApiClient({ fetcher });
  await checkLogQuery("t /", "u /?").queryFn({ signal: new AbortController().signal });
  expect(fetcher.mock.calls[0]?.[0]).toBe("/api/tasks/t%20%2F/work-units/u%20%2F%3F/check-log");
});
it("fetches the job and history from the daemon relay", async () => {
  const fetcher = vi.fn(async () => new Response("{}", { status: 200 }));
  configureApiClient({ fetcher });
  await cronDetailQuery("daily").queryFn({ signal: new AbortController().signal });
  expect(fetcher.mock.calls.map((call) => (call as unknown[])[0])).toEqual([
    "/api/cron-jobs/daily",
    "/api/cron-jobs/daily/runs",
  ]);
});
