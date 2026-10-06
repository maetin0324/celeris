import type {
  Level,
  ReviewResult,
  RoutingOutcome,
  RunRoutingAudit,
  TaskFeatures,
  TaskRoutingView,
  TierSource,
} from "~/celeris/types";

/**
 * タスク詳細の「ルーティング」パネル（celeris ADR-0069 D5、`GET /tasks/{id}/routing`）の表示用の純粋関数。
 * celeris が記録した値をそのまま並べるだけで、lane や担当を GUI 側で再計算しない。
 */

export const ROUTING_PANEL_LABEL = "ルーティング";

/** 値が無いところに出す記号。 */
const NONE = "—";

/** 表示の中心にする run（いちばん新しい run）。run が無ければ null。 */
export function latestRoutingRun(view: TaskRoutingView | null | undefined): RunRoutingAudit | null {
  return view?.runs.at(-1) ?? null;
}

/** 1 行の要約: `org / harness / lane / model`（無い値は `—`）。 */
export function routingSummaryParts(run: RunRoutingAudit): { label: string; value: string }[] {
  return [
    { label: "担当", value: run.org_node ?? NONE },
    { label: "harness", value: run.harness ?? NONE },
    { label: "lane", value: run.lane ?? NONE },
    { label: "model", value: run.model ?? NONE },
  ];
}

export function routingSummaryLine(run: RunRoutingAudit): string {
  return routingSummaryParts(run)
    .map((p) => p.value)
    .join(" / ");
}

const FEATURE_AXES: { key: keyof TaskFeatures; label: string }[] = [
  { key: "judgment", label: "判断の重さ" },
  { key: "ambiguity", label: "曖昧さ" },
  { key: "verifiability", label: "検証しやすさ" },
  { key: "reversibility", label: "やり直しやすさ" },
  { key: "consequence", label: "失敗の損失" },
  { key: "context_size", label: "文脈の大きさ" },
  { key: "tool_intensity", label: "道具の使用" },
  { key: "expected_length", label: "想定の長さ" },
  { key: "cross_cutting", label: "領域の横断" },
];

const LEVEL_LABEL: Record<Level, string> = { low: "低", medium: "中", high: "高" };

/** features の表（軸の名前・API のキー・段階）。 */
export function featureRows(features: TaskFeatures): { key: string; label: string; level: string }[] {
  return FEATURE_AXES.map(({ key, label }) => ({ key, label, level: LEVEL_LABEL[features[key]] ?? features[key] }));
}

const TIER_SOURCE_LABEL: Record<TierSource, string> = {
  human: "人の明示",
  system: "celeris の固定",
  hint: "policy（LLM の指定はヒントのみ）",
  default: "policy",
};

/** lane を誰が決めたか（`Task.routing.tier_source`）。 */
export function tierSourceLabel(source: TierSource | undefined): string {
  return TIER_SOURCE_LABEL[source ?? "default"];
}

/** CoS / 計画 / 委譲が書いたが celeris が使わなかった担当の注記。無ければ null。 */
export function droppedAssigneeNote(view: TaskRoutingView | null | undefined): string | null {
  const dropped = view?.routing?.dropped_assignee;
  return dropped ? `CoS などが提案した担当「${dropped}」は使わず、celeris が決定的に選びました` : null;
}

export function formatCostUsd(cost: number | null | undefined): string {
  if (cost == null) return NONE;
  return `$${cost < 0.01 && cost > 0 ? cost.toFixed(4) : cost.toFixed(2)}`;
}

export function formatTokens(input: number | null | undefined, output: number | null | undefined): string {
  if (input == null && output == null) return NONE;
  return `入力 ${(input ?? 0).toLocaleString("en-US")} / 出力 ${(output ?? 0).toLocaleString("en-US")}`;
}

export function formatWallMs(ms: number | null | undefined): string {
  if (ms == null) return NONE;
  if (ms < 1000) return `${ms} ms`;
  const secs = Math.round(ms / 1000);
  if (secs < 60) return `${secs} 秒`;
  const mins = Math.floor(secs / 60);
  return `${mins} 分 ${secs % 60} 秒`;
}

export function reviewResultLabel(review: ReviewResult | null | undefined): string {
  if (!review) return NONE;
  if (review.passed) return "合格";
  const failed = review.failed_criteria ?? [];
  return failed.length > 0 ? `不合格（条件 ${failed.map((i) => `#${i}`).join(", ")}）` : "不合格";
}

/** エスカレーションの履歴（`escalation` か `escalation_audit` が付いた run。古い順）。 */
export function escalationHistory(view: TaskRoutingView | null | undefined): { runId: string; text: string }[] {
  return (view?.runs ?? [])
    .filter((r) => r.escalation || r.escalation_audit)
    .map((r) => ({ runId: r.run_id, text: escalationLine(r) ?? "" }));
}

/**
 * Phase 3（ADR 2026-10-04-multi-objective-model-routing §5）: run 間 escalation の 1 行。
 * 構造化監査（`escalation_audit`）があれば requested → selected と理由・回数を出し、
 * 無ければ旧 event の自由文字列 `escalation` をそのまま出す。両方無ければ null。
 */
export function escalationLine(run: RunRoutingAudit): string | null {
  const audit = run.escalation_audit;
  if (audit) {
    const moved = audit.selected_lane !== audit.requested_lane;
    const lanes = `${audit.requested_lane} → ${audit.selected_lane}`;
    const count = moved
      ? `（品質失敗 ${audit.counted_failures} 回で 1 段）`
      : `（${audit.counted_failures} 回でも上げない）`;
    return `${lanes} ${audit.reason}${count}`;
  }
  return run.escalation ?? null;
}

/**
 * Phase 3: outcome の状態（§6「未レビュー/中断は null、false や品質 0 とみなさない」）。
 * `outcome_state`（celeris が決めた値）を人の語にする。run 全体と outcome 本体を別々に持たせるのは、
 * `not_recorded`（outcome 自体が未追記）を「未レビュー」と見分けられなくするため。
 */
export const OUTCOME_STATE_LABEL = {
  not_recorded: "未追記（まだ outcome が無い）",
  unreviewed: "未レビュー（review・acceptance の判定待ち）",
  judged: "判定済み",
} as const;

export function outcomeStateLabel(run: RunRoutingAudit): string {
  return OUTCOME_STATE_LABEL[run.outcome_state ?? "not_recorded"];
}

/** Phase 3: outcome 本体（`routing_outcome_recorded`）の 1 行。未追記（null）は null。 */
export function outcomeLine(outcome: RoutingOutcome | null | undefined): string | null {
  if (!outcome) return null;
  const parts: string[] = [];
  if (outcome.review_passed === true) parts.push("review 合格");
  else if (outcome.review_passed === false) parts.push("review 不合格");
  if (outcome.acceptance_passed === true) parts.push("acceptance 合格");
  else if (outcome.acceptance_passed === false) parts.push("acceptance 不合格");
  if ((outcome.failed_criterion_ids ?? []).length > 0)
    parts.push(`失敗条件 ${outcome.failed_criterion_ids?.join(", ")}`);
  if (outcome.failure_class) parts.push(`（${outcome.failure_class}）`);
  const reward = outcome.reward == null ? "reward 未判定" : `reward ${outcome.reward.toFixed(3)}`;
  return `${parts.join("・") || "判定なし（未レビュー/中断）"}${outcome.supersedes ? `、旧 ${outcome.supersedes} を置き換え` : ""} ${reward}`;
}

/** Phase 3: 実行 lane（この run が実際に走った lane `run.lane`）と希望 lane（trace の `requested_lane`）の差。同じなら null。 */
export function executedLaneNote(run: RunRoutingAudit): string | null {
  const actual = run.lane;
  const requested = (run.requests ?? [])[0]?.trace.requested_lane;
  if (!actual || !requested) return null;
  return actual === requested ? null : `希望 ${requested} → 実行 ${actual}`;
}
