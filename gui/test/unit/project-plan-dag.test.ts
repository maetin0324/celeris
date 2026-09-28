import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { describe, expect, it } from "vitest";
import type { PlanDagNode, ProjectPlanDagView } from "~/celeris/types";
import { ProjectPlanDag } from "~/components/ProjectPlanDag";

const node = (key: string, over: Partial<PlanDagNode> = {}): PlanDagNode => ({
  key,
  title: `title-${key}`,
  depends_on: [],
  milestone_id: `m-${key}`,
  task_id: `t-${key}`,
  work_units_done: 0,
  work_units_total: 0,
  children_done: 0,
  children_total: 0,
  quota: [],
  ...over,
});

function render(plan: ProjectPlanDagView): string {
  const router = createMemoryRouter(
    [
      {
        path: "/",
        element: createElement(ProjectPlanDag, { plan, milestones: [], renderReview: () => null }),
      },
    ],
    { initialEntries: ["/"] },
  );
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

/** ADR-0077 D5: DAG の節点の途中目標バッジは状態ごとの色とラベルで出す（in_progress = 進行中）。 */
describe("ProjectPlanDag node milestone badge", () => {
  it("in_progress を「進行中」の warning で出す", () => {
    const html = render({
      current_version: 1,
      nodes: [
        node("survey", { milestone_status: "in_progress", task_status: "running" }),
        node("poc", { depends_on: ["survey"], milestone_status: "approved" }),
      ],
    });
    const badge = html.match(/<span[^>]*data-milestone-status="in_progress"[^>]*>([^<]*)<\/span>/);
    expect(badge?.[0]).toBeDefined();
    expect(badge?.[1]).toBe("進行中");
    expect(badge?.[0]).toMatch(/warning/);
    const approved = html.match(/<span[^>]*data-milestone-status="approved"[^>]*>([^<]*)<\/span>/);
    expect(approved?.[1]).toBe("承認済み");
  });
});
