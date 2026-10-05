import { expect, test } from "vitest";
import type { Graph } from "../../api/generated/types";
import { layoutGraph, NODE_HEIGHT, NODE_WIDTH } from "./graph-layout";

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

test("graph layout keeps nodes apart and inside the canvas", () => {
  const graph: Graph = {
    nodes: [
      { id: "A", title: "a", status: "ready", kind: "execute" },
      { id: "B", title: "b", status: "done", kind: "execute" },
      { id: "C", title: "c", status: "failed", kind: "execute" },
    ],
    edges: [
      { from: "A", to: "B", kind: "depends_on" },
      { from: "A", to: "C", kind: "depends_on" },
    ],
  };
  const result = layoutGraph(graph);
  for (const node of result.nodes) {
    expect(node.x + NODE_WIDTH).toBeLessThanOrEqual(result.width);
    expect(node.y + NODE_HEIGHT).toBeLessThanOrEqual(result.height);
  }
  const [b, c] = result.nodes.filter((node) => node.id !== "A");
  expect(Math.abs((b?.y ?? 0) - (c?.y ?? 0))).toBeGreaterThanOrEqual(NODE_HEIGHT);
  const edge = result.edges.find((item) => item.to === "B");
  expect(edge?.x2).toBe(b?.x);
  expect(edge?.y2).toBe((b?.y ?? 0) + NODE_HEIGHT / 2);
});
