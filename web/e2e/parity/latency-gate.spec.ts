import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { expect, test } from "@playwright/test";
import { QueryClient } from "@tanstack/react-query";
import type { EventRow } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { keysForTaskEvent, projectFallbackKeys, resolveProjectId } from "../../api/realtime/invalidation-map";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { waitForBootIdle } from "../latency/boot-idle.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { recordLatency } from "../support/latency-results";
import { makeTmpDir } from "../support/tmp-dir";

const task = {
  id: "T1",
  title: "一件目",
  status: "ready",
  kind: "execute",
  category: "work",
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
};

function harness() {
  const dir = makeTmpDir("celeris-p5-latency-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({
    token: FIXTURE_TOKEN,
    fixtures: {
      ...defaultFixtures,
      "/api/v1/tasks": { items: [task], total: 1, next_cursor: null, counts_by_status: { ready: 1 } },
      "/api/v1/tasks/T1": task,
    },
  });
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

const row = (id: number, taskId: string, type: EventRow["event"]["type"]): EventRow =>
  ({
    id,
    seq: id,
    task_id: taskId,
    ts: "2026-09-30T00:00:00Z",
    event: { type },
  }) as EventRow;

test("parity-x: 遅延 10 s で遷移が止まらない（baseline の 7 経路）", async ({ page }) => {
  test.setTimeout(120_000);
  const h = harness();
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/`);
    // goto 直後の起動の long task を計測に含めない（ADR-0081 付記、人の決定 b）。
    await waitForBootIdle(page);
    const paths = ["/inbox", "/tasks", "/projects", "/reports", "/org", "/knowledge", "/daemon"];
    const measurements: Array<{ path: string; url: number; heading: number }> = [];
    h.daemon.setDelay(10000);
    for (const target of paths) {
      const link = page.locator(`nav[aria-label="主要"] a[href="${target}"]`);
      const start = performance.now();
      await link.click();
      await expect(page).toHaveURL(`${gateway.base}${target}`);
      const url = performance.now() - start;
      await expect(page.locator("main h1")).toBeVisible();
      const heading = performance.now() - start;
      measurements.push({ path: target, url, heading });
      expect(url, `${target} URL`).toBeLessThanOrEqual(300);
      expect(heading, `${target} heading`).toBeLessThanOrEqual(300);
    }
    recordLatency({ kind: "baseline-seven-routes", delay: 10000, measurements });
  } finally {
    await h.close(gateway);
  }
});

test("parity-x: SSE イベントごとの再取得本数と H1 project fallback", async ({ page }) => {
  const h = harness();
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id='T1']")).toBeVisible();
    await expect.poll(() => h.daemon.streamClients).toBeGreaterThan(0);
    const count = (route: string) => h.daemon.requests.filter((r) => r.path === route).length;
    const events: Array<{ event: string; tasks: number; inbox: number; daemon: number }> = [];
    const samples = [
      [1, "unrelated", "worker_progress"],
      [2, "T1", "worker_progress"],
      [3, "T1", "transitioned"],
      [4, "unknown", "transitioned"],
    ] as const;
    for (const [id, taskId, type] of samples) {
      const before = [count("/api/v1/tasks"), count("/api/v1/inbox/items"), count("/api/v1/daemon")];
      h.daemon.sendEvent("task.event", row(id, taskId, type));
      await page.waitForTimeout(700); // 250 ms coalescing window plus transport/render time.
      events.push({
        event: `${taskId}:${type}`,
        tasks: count("/api/v1/tasks") - before[0],
        inbox: count("/api/v1/inbox/items") - before[1],
        daemon: count("/api/v1/daemon") - before[2],
      });
    }
    const cache = new QueryClient();
    cache.setQueryData(taskKeys.list(), { items: [task] });
    const fallbackCount = samples.reduce((sum, [id, taskId, type]) => {
      const event = row(id, taskId, type);
      const projectId = resolveProjectId(cache, event.task_id, event.event);
      const keys = keysForTaskEvent({ taskId: event.task_id, event: event.event, projectId });
      return (
        sum +
        Number(
          projectId === null &&
            projectFallbackKeys().every((key) => keys.some((actual) => JSON.stringify(actual) === JSON.stringify(key))),
        )
      );
    }, 0);
    recordLatency({
      kind: "event-refetch",
      events,
      projectFallbackCount: fallbackCount,
      projectFallbackSampleEvents: samples.length,
    });
    expect(events[0].tasks).toBe(0);
    expect(events[1].tasks).toBe(0);
    expect(events[2].tasks).toBe(1);
    expect(events[3].tasks).toBe(1);
    expect(fallbackCount).toBe(1);
  } finally {
    await h.close(gateway);
  }
});

test("parity-x: 遅延 5 s と 2 s tick で /tasks から詳細へ進み要求が増え続けない", async ({ page }) => {
  test.setTimeout(60_000);
  const h = harness();
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id='T1']")).toBeVisible();
    h.daemon.setDelay(5000);
    const start = performance.now();
    await page.locator("[data-task-id='T1'] a[href*='/tasks/T1']").first().click();
    await expect(page).toHaveURL(`${gateway.base}/tasks/T1`);
    await expect(page.locator("main h1")).toBeVisible();
    const heading = performance.now() - start;
    const before = h.daemon.requests.length;
    const abortedBefore = h.daemon.requests.filter((r) => r.aborted).length;
    await page.waitForTimeout(12_000);
    const middle = h.daemon.requests.length;
    const abortedAt12s = h.daemon.requests.filter((r) => r.aborted).length;
    await page.waitForTimeout(6_000);
    const after = h.daemon.requests.length;
    const abortedAt18s = h.daemon.requests.filter((r) => r.aborted).length;
    recordLatency({
      kind: "tick-during-delay",
      delay: 5000,
      tick: 2000,
      heading,
      requestsFirst12s: middle - before,
      requestsNext6s: after - middle,
      abortedFirst12s: abortedAt12s - abortedBefore,
      abortedNext6s: abortedAt18s - abortedAt12s,
    });
    expect(heading).toBeLessThanOrEqual(300);
    expect(after - middle).toBeLessThanOrEqual(middle - before);
    expect(abortedAt18s - abortedAt12s).toBe(0);
  } finally {
    await h.close(gateway);
  }
});
