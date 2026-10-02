import type { PlanDagNode, ProjectTaskView } from "../../api/generated/types";

// 案件詳細（P4-02）の計画 DAG の段と仕事の木。描画から切り離した純関数（features/projects の単体テスト）。

/** DAG の段（依存の最長路の深さ）。循環や未知の依存は 0 段扱いにして止まる。 */
export function dagLayers(nodes: readonly PlanDagNode[]): PlanDagNode[][] {
  const byKey = new Map(nodes.map((node) => [node.key, node]));
  const depth = new Map<string, number>();
  const visiting = new Set<string>();
  const depthOf = (key: string): number => {
    const known = depth.get(key);
    if (known !== undefined) return known;
    const node = byKey.get(key);
    if (!node || visiting.has(key)) return 0;
    visiting.add(key);
    const d = node.depends_on.reduce((max, dep) => (byKey.has(dep) ? Math.max(max, depthOf(dep) + 1) : max), 0);
    visiting.delete(key);
    depth.set(key, d);
    return d;
  };
  const layers: PlanDagNode[][] = [];
  for (const node of nodes) {
    const d = depthOf(node.key);
    layers[d] = [...(layers[d] ?? []), node];
  }
  return layers.filter((layer) => layer !== undefined);
}

export type WorkTreeNode = { task: ProjectTaskView; children: WorkTreeNode[] };

/** parent_id で木にする。親が案件の外（一覧に無い）なら根に置く。 */
export function workTree(tasks: readonly ProjectTaskView[]): WorkTreeNode[] {
  const nodes = new Map(tasks.map((task) => [task.id, { task, children: [] as WorkTreeNode[] }]));
  const roots: WorkTreeNode[] = [];
  for (const node of nodes.values()) {
    const parent = node.task.parent_id ? nodes.get(node.task.parent_id) : undefined;
    if (parent && parent !== node) parent.children.push(node);
    else roots.push(node);
  }
  return roots;
}
