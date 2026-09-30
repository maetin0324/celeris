import { describe, expect, it } from "vitest";
import type { PlanDagNode, ProjectTaskView } from "../../api/generated/types";
import { dagLayers, workTree } from "./project-structure";

const node = (key: string, depends_on: string[] = []): PlanDagNode => ({
  key,
  depends_on,
  title: key,
  milestone_id: `M-${key}`,
  task_id: `T-${key}`,
  children_done: 0,
  children_total: 0,
  work_units_done: 0,
  work_units_total: 0,
});
const task = (id: string, parent_id?: string): ProjectTaskView => ({
  id,
  title: id,
  status: "ready",
  conversation: false,
  depends_on: [],
  parent_id: parent_id ?? null,
});

describe("dagLayers", () => {
  it("places nodes by the longest dependency path", () => {
    const layers = dagLayers([node("c", ["a", "b"]), node("a"), node("b", ["a"])]);
    expect(layers.map((layer) => layer.map((n) => n.key))).toEqual([["a"], ["b"], ["c"]]);
  });
  it("stops on cycles and unknown dependencies", () => {
    const layers = dagLayers([node("a", ["b"]), node("b", ["a"]), node("x", ["missing"])]);
    expect(
      layers
        .flat()
        .map((n) => n.key)
        .sort(),
    ).toEqual(["a", "b", "x"]);
  });
});

describe("workTree", () => {
  it("nests children and keeps orphans as roots", () => {
    const roots = workTree([task("T1"), task("T2", "T1"), task("T3", "T2"), task("T4", "outside")]);
    expect(roots.map((r) => r.task.id)).toEqual(["T1", "T4"]);
    expect(roots[0].children[0].children[0].task.id).toBe("T3");
  });
});
