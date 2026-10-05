import type {
  InboxItem,
  MilestoneStatus,
  PlanDagNode,
  ProjectDetail,
  ProjectTaskView,
} from "../../api/generated/types";

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

/** 木を表の行にする（字下げの深さ付き、親の直後に子）。 */
export function flattenTree(nodes: readonly WorkTreeNode[], depth = 0): { task: ProjectTaskView; depth: number }[] {
  return nodes.flatMap((node) => [{ task: node.task, depth }, ...flattenTree(node.children, depth + 1)]);
}

export type MilestoneGroup = {
  /** 途中目標の id。途中目標に属さない仕事の束は null。 */
  milestoneId: string | null;
  title: string;
  milestoneStatus: MilestoneStatus | null;
  /** 子 task の終わった数 / 全数（計画の節点が持つ値）。 */
  progress: { done: number; total: number } | null;
  roots: WorkTreeNode[];
};

/**
 * 案件 → 途中目標 → task の木（R10）。根の task を途中目標ごとに束ねる。
 * 途中目標の並びは計画 DAG の段の順、計画に無い途中目標はその後、途中目標の無い仕事は最後。
 */
export function milestoneGroups(
  detail: Pick<ProjectDetail, "tasks" | "milestones" | "project_plan">,
): MilestoneGroup[] {
  const nodes = dagLayers(detail.project_plan?.nodes ?? []).flat();
  const nodeByTask = new Map(nodes.map((node) => [node.task_id, node]));
  const legacy = new Map(detail.milestones.map((m) => [m.id, m]));
  const groups = new Map<string | null, MilestoneGroup>();
  for (const node of nodes) {
    if (groups.has(node.milestone_id)) continue;
    groups.set(node.milestone_id, {
      milestoneId: node.milestone_id,
      title: node.title,
      milestoneStatus: node.milestone_status ?? null,
      progress: { done: node.children_done, total: node.children_total },
      roots: [],
    });
  }
  const unassigned: WorkTreeNode[] = [];
  for (const root of workTree(detail.tasks)) {
    const id = root.task.milestone_id ?? nodeByTask.get(root.task.id)?.milestone_id ?? null;
    if (id === null) {
      unassigned.push(root);
      continue;
    }
    let group = groups.get(id);
    if (!group) {
      const m = legacy.get(id);
      group = {
        milestoneId: id,
        title: m?.title ?? id,
        milestoneStatus: m?.status ?? null,
        progress: null,
        roots: [],
      };
      groups.set(id, group);
    }
    group.roots.push(root);
  }
  const result = [...groups.values()];
  if (unassigned.length > 0) {
    result.push({
      milestoneId: null,
      title: "途中目標に属さない仕事",
      milestoneStatus: null,
      progress: null,
      roots: unassigned,
    });
  }
  return result;
}

/** 案件一覧の「途中目標の進み」: 達成した途中目標 / 中止・再設計を除いた全数。計画も途中目標も無ければ null。 */
export function milestoneProgress(
  detail: Pick<ProjectDetail, "milestones" | "project_plan">,
): { reached: number; total: number } | null {
  const planned = detail.project_plan?.nodes ?? [];
  const statuses: (MilestoneStatus | null)[] =
    planned.length > 0 ? planned.map((node) => node.milestone_status ?? null) : detail.milestones.map((m) => m.status);
  const counted = statuses.filter((status) => status !== "cancelled" && status !== "redesigned");
  if (counted.length === 0) return null;
  return { reached: counted.filter((status) => status === "reached").length, total: counted.length };
}

/** 判断待ちの数を案件ごとに数える（受信箱の項目の project_id）。 */
export function pendingByProject(items: readonly Pick<InboxItem, "project_id">[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const item of items) {
    if (item.project_id) counts.set(item.project_id, (counts.get(item.project_id) ?? 0) + 1);
  }
  return counts;
}

/** 判断待ちの数を task ごとに数える（項目の task と、止めている task）。 */
export function pendingByTask(items: readonly Pick<InboxItem, "task" | "blocking">[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const item of items) {
    const ids = new Set([item.task?.id, ...(item.blocking?.tasks ?? []).map((t) => t.id)]);
    for (const id of ids) if (id) counts.set(id, (counts.get(id) ?? 0) + 1);
  }
  return counts;
}
