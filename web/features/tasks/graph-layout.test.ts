import { expect, test } from "vitest";
import type { Graph } from "../../api/generated/types";
import { layoutGraph } from "./graph-layout";

test("graph layout stays finite with cycles and ignores dangling edges", () => {
  const graph: Graph = {
    nodes: [
      { id: "T1", title: "first", status: "ready", kind: "execute" },
      { id: "T2", title: "second", status: "running", kind: "execute" },
    ],
    edges: [
      { from: "T1", to: "T2", kind: "depends_on" },
      { from: "T2", to: "T1", kind: "depends_on" },
      { from: "missing", to: "T1", kind: "depends_on" },
    ],
  };
  const result = layoutGraph(graph);
  expect(result.nodes).toHaveLength(2);
  expect(result.edges).toHaveLength(2);
  expect(result.width).toBeGreaterThan(0);
  expect(result.height).toBeGreaterThan(0);
  expect(result.nodes.every((node) => Number.isFinite(node.x) && Number.isFinite(node.y))).toBe(true);
});
