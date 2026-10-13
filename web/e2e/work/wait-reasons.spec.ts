import { expect, test } from "@playwright/test";
import type { TaskList } from "../../api/generated/types";
import { richFixtures } from "../support/fake-daemon.mjs";
import { startFixtureGateway } from "../support/fixture-gateway";

test("board and task list show runnable and waiting tasks with links on mobile", async ({ page }) => {
  const original = richFixtures()["/api/v1/tasks"] as TaskList;
  const items = original.items.slice(0, 2).map((item, i) => ({
    ...item,
    id: i ? "T2" : "T1",
    title: i ? "Blocked on child" : "Ready task",
    status: "ready" as const,
    runnable: !i,
    waits: i ? [{ kind: "children" as const, task_ids: ["T3"], unit_keys: ["build"] }] : [],
  }));
  const gateway = await startFixtureGateway({
    fixtures: {
      "/api/v1/tasks": (url: URL) => {
        const runState = url.searchParams.get("run_state");
        const visible =
          runState === "runnable"
            ? items.filter((x) => x.runnable)
            : runState === "waiting"
              ? items.filter((x) => !x.runnable)
              : items;
        return { ...original, items: visible, total: visible.length, next_cursor: null };
      },
    },
  });
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${gateway.base}/board`);
    await expect(page.getByTestId("task-wait-reason")).toContainText("子 1 件待ち");
    await expect(page.getByTestId("task-wait-reason").getByRole("link", { name: "T3 を開く" })).toHaveAttribute(
      "href",
      /tasks\/T3/,
    );
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page
      .getByTestId("board-run-state")
      .getByRole("link", { name: /走れる/ })
      .click();
    await expect(page.locator("[data-task-id]")).toHaveCount(1);
    await page.goto(`${gateway.base}/tasks`);
    await expect(page.getByTestId("task-wait-reason")).toContainText("子 1 件待ち");
    await page.getByTestId("tasks-run-state").getByRole("link", { name: "待ち" }).click();
    await expect(page).toHaveURL(/run_state=waiting/);
    await expect(page.locator("[data-task-id]")).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally {
    await gateway.close();
  }
});
