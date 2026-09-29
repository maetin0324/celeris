import type {
  Action,
  CostOfReversal,
  DecisionInboxItem,
  DecisionKind,
  DecisionPathEntry,
  ExecutionView,
  PlanGateAction,
  RollupMetrics2,
  Status,
  TaskTreeNode,
  TaskTreeView,
  TimelineItem,
  TreeLimitsUsage,
  TreeNodePhase,
  TreeNodeStall,
} from "~/celeris/types";
import type { Tone } from "~/components/ui/tone";
import { formatDuration } from "~/lib/time-delta";

/**
 * celeris ADR-0079 D14（Phase R4b）: task の木・決定の受信箱・計画の承認・「理由なく止まっています」の表示の判定。
 * 純粋関数だけ（値は celeris が決めたものをそのまま並べる。押せるかどうかの最終判断は celeris の 409 / 422）。
 */

/** 木の節点の導出値（`TaskTreeNode.phase`。celeris の `TreeNodePhase`）の語。 */
export const TREE_NODE_PHASE_LABEL: Record<TreeNodePhase, string> = {
  planning: "計画中",
  executing: "実行中",
  repairing: "修復中",
  verifying: "検証中",
  awaiting_human: "確認待ち",
  awaiting_children: "子 task の完了待ち",
  awaiting_plan_approval: "計画の承認待ち",
  held_on_decision: "人の決定待ち",
  blocked_infra: "基盤の失敗で停止",
};

export const TREE_NODE_PHASE_TONE: Record<TreeNodePhase, Tone> = {
  planning: "info",
  executing: "primary",
  repairing: "warning",
  verifying: "teal",
  awaiting_human: "warning",
  awaiting_children: "info",
  awaiting_plan_approval: "warning",
  held_on_decision: "warning",
  blocked_infra: "danger",
};

/** D10 / R3b 付記 9.: 生存確認の「理由なし」の分類（`NodeLiveness.reason`）の語。知らない値はそのまま出す。 */
const STALL_REASON_LABEL: Record<string, string> = {
  child_missing: "待っている子 task が見つからない",
  decision_released: "決定は答え済みなのに次の計画が起きない",
  nothing_runnable: "走れる unit も名指しの待ちも無い",
  blocked_unit: "task は ready なのに unit が止まったまま",
  replans_exhausted: "立て直す余地（replan）も上限の決定も無い",
};

export const STALL_HEADLINE = "理由なく止まっています";

/** 「理由なく止まっています（<分類>）」の 1 行。 */
export function stallText(stall: TreeNodeStall): string {
  const reason = stall.reason ? (STALL_REASON_LABEL[stall.reason] ?? stall.reason) : null;
  return reason ? `${STALL_HEADLINE}（${reason}）` : STALL_HEADLINE;
}

/**
 * タスク詳細（木を引かない画面）での「今も理由なく止まっている」: 最後の event が `stall_detected` で終端でない
 * （celeris の `tree_view::current_stall` と同じ読み。R3b 付記 9. の「最後の event が StallDetected の間」）。
 */
export function currentStallFromTimeline(items: readonly TimelineItem[], status: Status): TreeNodeStall | null {
  if (status === "done" || status === "failed" || status === "cancelled") return null;
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind !== "event") continue;
    if (item.event.type !== "stall_detected") return null;
    return { reason: item.event.reason ?? "", since: item.event.since || null, detail: item.event.detail };
  }
  return null;
}

/** 止まっている理由の種類（D10 の分類）。`null` = 止まっていない（走っている・走れる・人の確認待ちなど別の画面で出る）。 */
export type NodeHold =
  | { kind: "stall"; text: string; detail: string }
  | { kind: "decision"; text: string }
  | { kind: "infra"; text: string }
  | { kind: "plan_approval"; text: string };

export const HOLD_TEXT = {
  decision: "人の決定を待っています（受信箱の「決定」で答えると再開します）",
  infra: "子 task が基盤の失敗で 2 回倒れたため止めています（再試行は人が選びます）",
  plan_approval: "計画の承認を待っています（承認するまで unit は 1 つも起きません）",
} as const;

/** 木の節点の「なぜ止まっているか」。stall（理由なし）を最優先に、名指しの待ち（決定・基盤・承認）を出す。 */
export function treeNodeHold(node: Pick<TaskTreeNode, "phase" | "stall" | "status">): NodeHold | null {
  if (node.status === "done" || node.status === "failed" || node.status === "cancelled") return null;
  if (node.stall) return { kind: "stall", text: stallText(node.stall), detail: node.stall.detail };
  switch (node.phase) {
    case "held_on_decision":
      return { kind: "decision", text: HOLD_TEXT.decision };
    case "blocked_infra":
      return { kind: "infra", text: HOLD_TEXT.infra };
    case "awaiting_plan_approval":
      return { kind: "plan_approval", text: HOLD_TEXT.plan_approval };
    default:
      return null;
  }
}

/**
 * タスク詳細の概要（木を引かない）での同じ判定。stall は timeline から、名指しの待ちは Execution 節の
 * `phase` / `plan_approval` と unit の `blocked_reason` から（celeris の `tree_view::node_phase` と同じ優先順）。
 */
export function taskHold(
  status: Status,
  execution: ExecutionView | null | undefined,
  stall: TreeNodeStall | null,
): NodeHold | null {
  if (status === "done" || status === "failed" || status === "cancelled") return null;
  if (stall) return { kind: "stall", text: stallText(stall), detail: stall.detail };
  if (execution?.plan_approval || execution?.phase === "awaiting_plan_approval") {
    return { kind: "plan_approval", text: HOLD_TEXT.plan_approval };
  }
  const units = (execution?.plan?.work_units ?? []).filter((u) => u.status === "blocked");
  if (units.some((u) => u.blocked_reason === "infra")) return { kind: "infra", text: HOLD_TEXT.infra };
  if (units.some((u) => u.blocked_reason === "decision")) return { kind: "decision", text: HOLD_TEXT.decision };
  return null;
}

/**
 * D14: 決定の path のパンくず（「browser capability › Phase 2 › P2-B」）。root の題名から始め、各段の段階・
 * 子の題名・unit を順に並べる（同じ語が続けば 1 つにする）。
 */
export function decisionBreadcrumb(path: readonly DecisionPathEntry[]): string {
  const parts: string[] = [];
  const push = (s: string | null | undefined) => {
    const v = s?.trim();
    if (v && parts[parts.length - 1] !== v) parts.push(v);
  };
  for (const entry of path) {
    push(entry.title);
    push(entry.stage);
    push(entry.unit);
  }
  return parts.join(" › ");
}

export const COST_OF_REVERSAL_LABEL: Record<CostOfReversal, string> = { low: "小", medium: "中", high: "大" };
export const COST_OF_REVERSAL_TONE: Record<CostOfReversal, Tone> = {
  low: "success",
  medium: "warning",
  high: "danger",
};

export const DECISION_KIND_LABEL: Record<DecisionKind, string> = {
  choice: "選択",
  leaf_too_large: "leaf が大きすぎる",
  limit: "上限",
  plan_invalid: "計画が不正",
};

/** 待っているもの（`needed_before`: `<unit key>` / `stage:<key>` / `self`）の 1 行。 */
export function neededBeforeText(needed: readonly string[]): string {
  if (needed.length === 0) return "なし（答えを待たずに進みます）";
  return needed
    .map((n) => (n === "self" ? "この task 自身" : n.startsWith("stage:") ? `段階 ${n.slice(6)}` : `unit ${n}`))
    .join("、");
}

/** 経過時間（受信箱の `age_secs`）。 */
export function decisionAgeText(ageSecs: number): string {
  return `${formatDuration(Math.max(0, ageSecs))}前`;
}

/** 選択肢の 1 つ（推奨に印）。`key = ""` は自由記述（`kind = choice` のときだけ。celeris は `other` と記録する）。 */
export interface DecisionChoice {
  key: string;
  label: string;
  consequence: string | null;
  recommended: boolean;
}

export const FREE_TEXT_CHOICE_LABEL = "自由記述（下の欄に書いた答えで進める）";

/**
 * 答えの選択肢（押せるものの出し分け）: 決定の選択肢に推奨の印を付け、`kind = choice` だけ自由記述を足す
 * （daemon の決定〈leaf_too_large / limit / plan_invalid〉は効き目が選択肢で決まるので option 必須。R3a 付記 8.）。
 */
export function decisionChoices(item: Pick<DecisionInboxItem, "kind" | "options" | "recommended">): DecisionChoice[] {
  const choices: DecisionChoice[] = item.options.map((o) => ({
    key: o.key,
    label: o.label,
    consequence: o.consequence ?? null,
    recommended: o.key === item.recommended,
  }));
  if (item.kind === "choice") {
    choices.push({ key: "", label: FREE_TEXT_CHOICE_LABEL, consequence: null, recommended: false });
  }
  return choices;
}

/** 計画の承認の 3 つの操作（D8）。 */
export const PLAN_GATE_ACTIONS: readonly PlanGateAction[] = ["approve", "replan", "withdraw"];

export const PLAN_GATE_ACTION_LABEL: Record<PlanGateAction, string> = {
  approve: "この計画で進める",
  replan: "計画を立て直す（指示が必須）",
  withdraw: "取り下げる",
};

/** 押せる計画の承認の操作: celeris が `actions` に `plan_gate` を入れたときだけ 3 つ（入れなければ出さない）。 */
export function planGateActions(actions: readonly Action[]): readonly PlanGateAction[] {
  return actions.includes("plan_gate") ? PLAN_GATE_ACTIONS : [];
}

const PLAN_REASON_LABEL: Record<string, string> = {
  decisions: "人の決定を含む",
  review_human: "人の確認を挟む段階がある",
  near_limit: "上限に近い",
};

/** 承認を求めた理由（`decisions:<key>,…` / `review_human:<stage>` / `near_limit:<設定名>:<値>/<上限>`）の 1 行ずつ。 */
export function planApprovalReasonLines(reasons: readonly string[]): string[] {
  return reasons.map((r) => {
    const i = r.indexOf(":");
    const head = i < 0 ? r : r.slice(0, i);
    const rest = i < 0 ? "" : r.slice(i + 1);
    const label = PLAN_REASON_LABEL[head];
    if (!label) return r;
    return rest ? `${label}（${rest}）` : label;
  });
}

/** 金額（定価。`cost_usd_complete = false` なら下限と明示）。 */
export function rollupCostText(m: Pick<RollupMetrics2, "cost_usd" | "cost_usd_complete">): string {
  const amount = `$${m.cost_usd.toFixed(2)}`;
  return m.cost_usd_complete ? amount : `${amount} 以上（単価不明のモデルあり）`;
}

const ROLE_ORDER = ["planner", "worker", "reviewer", "wrap_up"];

/** role ごとの run（reviewer を含む）。「planner 1 / worker 3 / reviewer 2」。run が無ければ「run なし」。 */
export function runsByRoleText(m: Pick<RollupMetrics2, "runs_by_role">): string {
  const entries = Object.entries(m.runs_by_role ?? {}).filter(([, n]) => n > 0);
  if (entries.length === 0) return "run なし";
  entries.sort(([a], [b]) => {
    const ia = ROLE_ORDER.indexOf(a);
    const ib = ROLE_ORDER.indexOf(b);
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
  });
  return entries.map(([role, n]) => `${role} ${n}`).join(" / ");
}

/** 壁時計（最初の run の開始 → 最後に終わった run の終わり）と実働。 */
export function wallClockText(m: Pick<RollupMetrics2, "wall_ms" | "busy_ms">): string {
  const wall = m.wall_ms == null ? "—" : formatDuration(Math.round(m.wall_ms / 1000));
  return `${wall}（実働 ${formatDuration(Math.round(m.busy_ms / 1000))}）`;
}

/** leaf の done / total と子 task の done / total。 */
export function progressText(
  m: Pick<RollupMetrics2, "leaves_done" | "leaves_total" | "child_tasks_done" | "child_tasks_total">,
): string {
  const leaves = `leaf ${m.leaves_done}/${m.leaves_total}`;
  return m.child_tasks_total > 0 ? `${leaves}・子 task ${m.child_tasks_done}/${m.child_tasks_total}` : leaves;
}

/** 木の上限の使用の 1 行（使った数 / 上限、0.8 以上は警告）。 */
export interface LimitUsageRow {
  key: string;
  label: string;
  used: number;
  max: number | null;
  near: boolean;
}

export function limitUsageRows(limits: TreeLimitsUsage): LimitUsageRow[] {
  const row = (key: string, label: string, used: number, max: number | null | undefined): LimitUsageRow => ({
    key,
    label,
    used,
    max: max ?? null,
    near: max != null && max > 0 && used / max >= 0.8,
  });
  return [
    row("leaves", "leaf", limits.leaves, limits.max_leaves),
    row("runs", "run（reviewer を除く）", limits.runs, limits.max_runs),
    row("replans", "replan", limits.replans, limits.max_replans),
    row("tokens", "トークン", limits.tokens, limits.max_tokens),
    row("open_decisions", "未回答の決定", limits.open_decisions, limits.max_open_decisions),
  ];
}

/** 木の節点を前順のまま、view の中の深さ（字下げの段数、0 始まり）付きで返す。 */
export function treeRows(view: TaskTreeView): { node: TaskTreeNode; indent: number }[] {
  const base = view.nodes[0]?.depth ?? 1;
  return view.nodes.map((node) => ({ node, indent: Math.max(0, node.depth - base) }));
}

/** 節点の unit を段階ごとにまとめる（段階の無い /1 の計画は 1 まとまり）。統合 unit は除く（leaf の行を折りたたむため）。 */
export function unitsByStage(
  node: TaskTreeNode,
): { stage: string | null; units: NonNullable<TaskTreeNode["units"]> }[] {
  const groups: { stage: string | null; units: NonNullable<TaskTreeNode["units"]> }[] = [];
  for (const u of node.units ?? []) {
    if (u.kind === "integrate") continue;
    const stage = u.stage ?? null;
    let g = groups.find((x) => x.stage === stage);
    if (!g) {
      g = { stage, units: [] };
      groups.push(g);
    }
    g.units.push(u);
  }
  return groups;
}

/**
 * D6: 成果の取り込み先。root は「成果の取り込み（main へ）」、木の子は「成果の取り込み（親『<題名>』の段階『<段階>』へ）」。
 * 木に属さない task は `null`（従来どおり「変更」タブの取り込み）。
 */
export function integrationTargetText(
  tree: { depth: number; parent_unit?: { stage: string } | null } | null | undefined,
  parentTitle: string | null,
  defaultBranch = "main",
): string | null {
  if (!tree) return null;
  if (!tree.parent_unit) return `成果の取り込み（${defaultBranch} へ）`;
  return `成果の取り込み（親『${parentTitle ?? "?"}』の段階『${tree.parent_unit.stage}』へ）`;
}

/**
 * D13 / D14: 案件の root task（`parent_id` が無く、対話でも裏方〈`support`〉でもない task。celeris の
 * `is_project_root_task` と同じ読み）。並びは celeris の `tasks[]` の順のまま。
 */
export function projectRootTasks<
  T extends { parent_id?: string | null; conversation: boolean; support?: string | null },
>(tasks: readonly T[]): T[] {
  return tasks.filter((t) => !t.parent_id && !t.conversation && !t.support);
}

/** 案件の root task の状態ごとの数（`root_totals.by_status`）の 1 行（「ready 2・done 3」）。 */
export function byStatusText(byStatus: Record<string, number> | null | undefined): string {
  const entries = Object.entries(byStatus ?? {}).filter(([, n]) => n > 0);
  return entries.length === 0 ? "—" : entries.map(([s, n]) => `${s} ${n}`).join("・");
}
