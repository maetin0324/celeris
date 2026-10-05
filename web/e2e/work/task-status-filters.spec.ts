import { expect, test } from "@playwright/test";
import type { Status, TaskList } from "../../api/generated/types";
import { richFixtures } from "../support/fake-daemon.mjs";
import { startFixtureGateway } from "../support/fixture-gateway";

test("状態 chip は即時に複数選択でき、URL・履歴・再読込と件数が一致する", async ({ page }) => {
  const original = richFixtures()["/api/v1/tasks"] as TaskList;
  const states: Status[] = ["ready", "ready", "running", "blocked"];
  const items = original.items.slice(0, 4).map((item, index) => ({
    ...item,
    status: states[index],
  }));
  let counts = { ready: 2, running: 1, blocked: 1 };
  const gateway = await startFixtureGateway({
    fixtures: {
      "/api/v1/tasks": (url: URL) => {
        const selected = url.searchParams.getAll("status");
        const visible = selected.length ? items.filter((item) => selected.includes(item.status)) : items;
        return { ...original, items: visible, total: visible.length, next_cursor: null };
      },
      "/api/v1/tasks/counts": () => ({ counts_by_status: counts }),
    },
  });
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/tasks`);
    const chips = page.getByTestId("tasks-status-chips");
    const ready = chips.getByRole("button", { name: /実行待ち/ });
    const running = chips.getByRole("button", { name: /実行中/ });
    await expect(chips).toContainText("実行待ち2");
    await expect(chips).toContainText("実行中1");
    await expect(chips).toContainText("停止中1");
    await expect(page.getByTestId("tasks-counts-scope")).toContainText("全タスク");
    await running.click();
    await expect(running).toHaveAttribute("aria-pressed", "true");
    await expect(page).toHaveURL(/status=running/);
    await expect(page.getByTestId("tasks-total")).toHaveText("1 件");
    await expect(page.locator("[data-task-id]")).toHaveCount(1);
    await ready.click();
    await expect(ready).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("tasks-total")).toHaveText("3 件");
    expect(new URL(page.url()).searchParams.getAll("status")).toEqual(["ready", "running"]);
    await page.reload();
    await expect(ready).toHaveAttribute("aria-pressed", "true");
    await expect(running).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("tasks-total")).toHaveText("3 件");
    await page.goBack();
    await expect(ready).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByTestId("tasks-total")).toHaveText("1 件");
    await page.goForward();
    await expect(ready).toHaveAttribute("aria-pressed", "true");
    await chips.getByRole("button", { name: "すべて" }).click();
    await expect(page.getByTestId("tasks-total")).toHaveText("4 件");
    await expect(chips.getByRole("button", { name: "すべて" })).toHaveAttribute("aria-pressed", "true");

    counts = { ready: 1, running: 2, blocked: 1 };
    gateway.daemon.sendEvent("task.event", {
      id: 1,
      seq: 1,
      task_id: items[0].id,
      ts: "2026-10-05T00:00:00Z",
      event: { type: "transitioned" },
    });
    await expect(chips).toContainText("実行待ち1");
    await expect(chips).toContainText("実行中2");
    expect(gateway.daemon.requests.some((request) => request.path === "/api/v1/tasks/counts")).toBe(true);
  } finally {
    await gateway.close();
  }
});
