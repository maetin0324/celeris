import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { TaskDetail, TaskExecutionView } from "../../api/generated/types";
import { fixtureFor, richFixtures } from "../support/fake-daemon.mjs";
import { startFixtureGateway } from "../support/fixture-gateway";

const schema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);

test("task tree shows ordered stages, WU dependency keys, and child wait reasons on mobile", async ({ page }) => {
  const fixtures = richFixtures();
  const detail = fixtureFor(schema.$defs.TaskDetail) as TaskDetail;
  detail.task = { ...detail.task, id: "T1", title: "統合", status: "ready" };
  detail.waits = [
    {
      kind: "work_units",
      units: [
        {
          key: "integrate-edges",
          stage: "close",
          waiting_on: ["edges", "remove", "dispatch-prune", "cos-route"],
          waiting_stage: "edges",
        },
        { key: "dispatch-prune", stage: "edges", waiting_on: ["cos-route"], waiting_stage: null },
      ],
    },
  ];
  detail.children = [{ id: "T2", title: "dispatch-prune", kind: "execute", status: "ready", actions: [] }];
  const child = fixtureFor(schema.$defs.TaskDetail) as TaskDetail;
  child.task = { ...child.task, id: "T2", title: "dispatch-prune", status: "ready" };
  child.runnable = false;
  child.waits = [
    {
      kind: "work_units",
      units: [{ key: "dispatch-prune", stage: "edges", waiting_on: ["cos-route"], waiting_stage: null }],
    },
  ];
  const execution = fixtures["/api/v1/tasks/T1/execution"] as TaskExecutionView;
  const plan = execution.plan;
  if (!plan) throw new Error("fixture execution plan missing");
  plan.plan.stages = [
    { key: "edges", title: "edges", kind: "implement" },
    { key: "remove", title: "remove", kind: "implement" },
    { key: "close", title: "close", kind: "implement" },
  ];
  const template = plan.work_units[0];
  if (!template) throw new Error("fixture work unit missing");
  plan.work_units = [
    {
      ...template,
      id: "wu-e",
      key: "integrate-edges",
      kind: "task",
      phase: "close",
      status: "pending",
      depends_on: ["edges", "remove", "dispatch-prune", "cos-route"],
      spec: { ...template.spec, key: "integrate-edges", title: "統合" },
    },
    {
      ...template,
      id: "wu-p",
      key: "dispatch-prune",
      kind: "task",
      phase: "edges",
      status: "pending",
      depends_on: ["cos-route"],
      spec: { ...template.spec, key: "dispatch-prune", title: "dispatch prune" },
    },
  ];
  fixtures["/api/v1/tasks/T1"] = detail;
  fixtures["/api/v1/tasks/T1/execution"] = execution;
  fixtures["/api/v1/tasks/T2"] = child;
  const gateway = await startFixtureGateway({ fixtures });
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${gateway.base}/tasks/T1`);
    const tree = page.getByTestId("task-tree");
    await expect(tree.locator("[data-testid=tree-stage]").nth(0)).toContainText("1/3");
    await expect(tree.locator("[data-testid=tree-stage]").nth(2)).toContainText("3/3");
    await expect(tree.locator("[data-key=integrate-edges]")).toContainText("edges、remove、dispatch-prune、cos-route");
    await expect(tree.locator("[data-key=dispatch-prune]")).toContainText("cos-route");
    await expect(tree.getByTestId("tree-child")).toContainText("cos-route");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  } finally {
    await gateway.close();
  }
});
