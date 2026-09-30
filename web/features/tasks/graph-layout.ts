import type { Graph } from "../../api/generated/types";

export type GraphLayout = {
  width: number;
  height: number;
  nodes: Array<{ id: string; title: string; status: string; x: number; y: number }>;
  edges: Array<{ from: string; to: string; x1: number; y1: number; x2: number; y2: number }>;
};

// The daemon owns graph membership. This is only a bounded, deterministic display layout.
export function layoutGraph(graph: Graph): GraphLayout {
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const incoming = new Map(graph.nodes.map((node) => [node.id, 0]));
  const outgoing = new Map(graph.nodes.map((node) => [node.id, [] as string[]]));
  for (const edge of graph.edges) {
    if (!byId.has(edge.from) || !byId.has(edge.to)) continue;
    outgoing.get(edge.from)?.push(edge.to);
    incoming.set(edge.to, (incoming.get(edge.to) ?? 0) + 1);
  }
  const queue = graph.nodes
    .filter((node) => incoming.get(node.id) === 0)
    .map((node) => node.id)
    .sort();
  const depth = new Map<string, number>();
  while (queue.length) {
    const id = queue.shift();
    if (!id) break;
    for (const next of outgoing.get(id) ?? []) {
      depth.set(next, Math.max(depth.get(next) ?? 0, (depth.get(id) ?? 0) + 1));
      incoming.set(next, (incoming.get(next) ?? 0) - 1);
      if (incoming.get(next) === 0) queue.push(next);
    }
    queue.sort();
  }
  // Cycles and dangling edges still get a readable column.
  const columns = new Map<number, string[]>();
  for (const node of [...graph.nodes].sort((a, b) => a.id.localeCompare(b.id))) {
    const level = Math.min(depth.get(node.id) ?? 0, graph.nodes.length);
    const column = columns.get(level) ?? [];
    column.push(node.id);
    columns.set(level, column);
  }
  const positioned = [...columns.entries()].flatMap(([level, ids]) =>
    ids.map((id, index) => {
      const node = byId.get(id);
      if (!node) throw new Error(`missing graph node: ${id}`);
      return { id, title: node.title, status: node.status, x: 24 + level * 248, y: 24 + index * 104 };
    }),
  );
  const positions = new Map(positioned.map((node) => [node.id, node]));
  const edges = graph.edges.flatMap((edge) => {
    const from = positions.get(edge.from);
    const to = positions.get(edge.to);
    return from && to
      ? [{ from: edge.from, to: edge.to, x1: from.x + 200, y1: from.y + 36, x2: to.x, y2: to.y + 36 }]
      : [];
  });
  return {
    width: Math.max(248, ...positioned.map((node) => node.x + 224)),
    height: Math.max(160, ...positioned.map((node) => node.y + 96)),
    nodes: positioned,
    edges,
  };
}
