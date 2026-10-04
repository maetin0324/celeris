import { describe, expect, it } from "vitest";
import type { PlanDagNode, ProjectTaskView } from "../../api/generated/types";
import {
  dagLayers,
  flattenTree,
  milestoneGroups,
  milestoneProgress,
  pendingByProject,
  pendingByTask,
  workTree,
} from "./project-structure";

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

describe("flattenTree", () => {
  it("lists parents before children with depth", () => {
    const rows = flattenTree(workTree([task("T1"), task("T2", "T1"), task("T3", "T2"), task("T4")]));
    expect(rows.map((r) => [r.task.id, r.depth])).toEqual([
      ["T1", 0],
      ["T2", 1],
      ["T3", 2],
      ["T4", 0],
    ]);
  });
});

describe("milestoneGroups", () => {
  it("groups root tasks by milestone in DAG order and puts loose tasks last", () => {
    const groups = milestoneGroups({
      project_plan: { nodes: [node("b", ["a"]), node("a")] } as never,
      milestones: [
        {
          id: "M-old",
          title: "以前の途中目標",
          status: "reached",
          seq: 1,
          project_id: "P1",
          created_at: "",
          updated_at: "",
        },
      ],
      tasks: [task("T-b"), task("T-a"), task("C1", "T-a"), { ...task("X"), milestone_id: "M-old" }, task("loose")],
    });
    expect(groups.map((g) => [g.milestoneId, g.roots.map((r) => r.task.id)])).toEqual([
      ["M-a", ["T-a"]],
      ["M-b", ["T-b"]],
      ["M-old", ["X"]],
      [null, ["loose"]],
    ]);
    expect(groups[2].title).toBe("以前の途中目標");
    expect(groups[0].roots[0].children[0].task.id).toBe("C1");
  });
});

describe("milestoneProgress", () => {
  it("counts reached milestones and skips cancelled ones", () => {
    const nodes = [
      { ...node("a"), milestone_status: "reached" as const },
      { ...node("b"), milestone_status: "in_progress" as const },
      { ...node("c"), milestone_status: "cancelled" as const },
    ];
    expect(milestoneProgress({ project_plan: { nodes } as never, milestones: [] })).toEqual({ reached: 1, total: 2 });
    expect(milestoneProgress({ project_plan: null, milestones: [] })).toBeNull();
  });
});

describe("pending counts", () => {
  const ref = (id: string) => ({ id, title: id, kind: "execute" as const, status: "blocked" as const, actions: [] });
  it("counts inbox items per project and per task", () => {
    const items = [
      { project_id: "P1", task: ref("T1"), blocking: { tasks: [ref("T1"), ref("T2")], units: [], summary: "" } },
      { project_id: "P1", task: null, blocking: { tasks: [ref("T2")], units: [], summary: "" } },
      { project_id: null, task: ref("T9"), blocking: { tasks: [], units: [], summary: "" } },
    ];
    expect([...pendingByProject(items)]).toEqual([["P1", 2]]);
    expect(Object.fromEntries(pendingByTask(items))).toEqual({ T1: 1, T2: 2, T9: 1 });
  });
});
