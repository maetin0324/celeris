import type { PlanDagNode, PlanNodeChange, PlanStopReason, ProjectPlanDecisionInput, QuotaUse } from "~/celeris/types";

/**
 * 案件ページの DAG（ADR-0074 D3.5、Phase F4b (h)）の表示だけの純粋関数。
 * 判断（Go が開いているか・止まっている理由・進み具合）は celeris が `GET /projects/{id}` の
 * `project_plan` に入れて返すので、ここでは並べ方と文言だけを決める（GUI 側で状態を推定しない）。
 */

/**
 * 節点を「層」に分ける（層 = 依存の最長経路の長さ。依存の無い節点が層 0）。層の中の並びは入力の順
 * （celeris の `plan.milestones` の順）のまま。知らない key への依存は無視する（表示だけなので落とさない）。
 * 循環は celeris の検証で起きないが、万一あれば残りを最後の層にまとめる。
 */
export function planLayers(nodes: readonly PlanDagNode[]): PlanDagNode[][] {
  const byKey = new Map(nodes.map((n) => [n.key, n]));
  const depth = new Map<string, number>();
  const visiting = new Set<string>();
  const depthOf = (key: string): number => {
    const known = depth.get(key);
    if (known !== undefined) return known;
    if (visiting.has(key)) return 0;
    visiting.add(key);
    const node = byKey.get(key);
    let d = 0;
    for (const dep of node?.depends_on ?? []) {
      if (byKey.has(dep)) d = Math.max(d, depthOf(dep) + 1);
    }
    visiting.delete(key);
    depth.set(key, d);
    return d;
  };
  const layers: PlanDagNode[][] = [];
  for (const n of nodes) {
    const d = depthOf(n.key);
    while (layers.length <= d) layers.push([]);
    layers[d]?.push(n);
  }
  return layers.filter((l) => l.length > 0);
}

/** モバイル幅の縦の一覧（トポロジカル順 = 層の順に並べたもの）。 */
export function planTopologicalOrder(nodes: readonly PlanDagNode[]): PlanDagNode[] {
  return planLayers(nodes).flat();
}

const STOP_REASON_LABEL: Record<PlanStopReason, string> = {
  awaiting_human: "途中確認待ち",
  question: "質問への回答待ち",
  failed: "失敗",
  awaiting_go: "前の途中目標の判定待ち",
  paused: "一時停止中",
};

export function planStopReasonLabel(reason: PlanStopReason): string {
  return STOP_REASON_LABEL[reason] ?? reason;
}

const CHANGE_LABEL: Record<PlanNodeChange, string> = {
  add: "追加",
  modify: "変更",
  remove: "外す",
  cancel: "取り下げ",
};

export function planChangeLabel(change: PlanNodeChange): string {
  return CHANGE_LABEL[change] ?? change;
}

/** 進み具合の 1 行（WU と子 Task。どちらも 0 件なら `null`）。 */
export function planProgressText(
  node: Pick<PlanDagNode, "work_units_done" | "work_units_total" | "children_done" | "children_total">,
): string | null {
  const parts: string[] = [];
  if (node.work_units_total > 0) parts.push(`WU ${node.work_units_done}/${node.work_units_total}`);
  if (node.children_total > 0) parts.push(`子 ${node.children_done}/${node.children_total}`);
  return parts.length > 0 ? parts.join("・") : null;
}

/** 使った quota の 1 行（窓ごとの `used_pct` の合計。値が無ければ `null`）。 */
export function planQuotaText(quota: readonly QuotaUse[] | undefined): string | null {
  const parts = (quota ?? [])
    .filter((q) => typeof q.used_pct === "number")
    .map((q) => `${q.source} ${q.window === "five_hour" ? "5h" : "7d"} ${(q.used_pct ?? 0).toFixed(1)}%`);
  return parts.length > 0 ? parts.join("・") : null;
}

/** 却下には理由が要る（celeris も 422 にするが、押す前に言う）。 */
export function projectPlanDecisionValid(decision: ProjectPlanDecisionInput, note: string): boolean {
  return decision !== "reject" || note.trim().length > 0;
}
