import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

function harness(fixtures: Record<string, unknown>) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-tasks-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...fixtures } });
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

const task = (id: string, title: string) => ({
  id,
  title,
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
});

test("parity: /tasks 絞り込み・検索・続き", async ({ page }) => {
  const calls: string[] = [];
  const fixture = (url: URL) => {
    calls.push(url.search);
    const cursor = url.searchParams.get("cursor");
    return cursor
      ? { items: [task("T2", "二件目")], total: 2, next_cursor: null, counts_by_status: { ready: 2 } }
      : { items: [task("T1", "一件目")], total: 2, next_cursor: "next", counts_by_status: { ready: 2 } };
  };
  const h = harness({ "/api/v1/tasks": fixture });
  const gateway = await h.start();
  try {
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.locator("[data-task-id='T1']")).toBeVisible();
    await page.getByRole("button", { name: "さらに読む" }).click();
    await expect(page.locator("[data-task-id='T2']")).toBeVisible();
    h.daemon.sendEvent("task.event", { cursor: 1, event: { id: 1, task_id: "T1", event: { type: "task_updated" } } });
    await expect(page.locator("[data-task-id='T2']")).toBeVisible();
    const search = page.getByRole("searchbox", { name: "検索" });
    await search.fill("計算");
    await page.getByRole("checkbox", { name: "ready" }).check();
    await page.getByLabel("並び").selectOption("created_desc");
    await page.locator("[data-testid='tasks-filter'] select[name='limit']").selectOption("20");
    await search.focus();
    await search.press("Enter");
    await expect(page).toHaveURL(/q=%E8%A8%88%E7%AE%97/);
    await expect(page).toHaveURL(/status=ready/);
    await expect(page).toHaveURL(/order=created_desc/);
    await expect(page).toHaveURL(/limit=20/);
    await expect(search).toBeFocused();
    await expect
      .poll(() => calls.some((query) => query.includes("q=%E8%A8%88%E7%AE%97") && query.includes("limit=20")))
      .toBe(true);
    await page.reload();
    await expect(page.getByRole("searchbox", { name: "検索" })).toHaveValue("計算");
    await expect(page.getByRole("checkbox", { name: "ready" })).toBeChecked();
    await expect(page).toHaveURL(/status=ready/);
    expect(calls.some((query) => query.includes("cursor=next"))).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(
      0,
    );
  } finally {
    await h.close(gateway);
  }
});

