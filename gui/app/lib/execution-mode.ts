import type { ExecutionGateDecision, ExecutionHintSpec, GateSource, Status, Task } from "~/celeris/types";

/**
 * タスク詳細の「実行の形を決め直す」（celeris ADR-0072「Phase F6 実装時の決定」、
 * `POST /tasks/{id}/execution/decompose` と `POST /tasks/{id}/retry` の `execution`）の表示用の純粋関数。
 * どの操作を**出すか**だけを決める（押せるかどうかの最終判断は celeris。409 / 422 はその文言を出す）。
 */

/** gate の判定の出どころ（なぜ atomic / compound になったか）。 */
export const GATE_SOURCE_LABEL: Record<GateSource, string> = {
  policy: "規則表",
  hint: "CoS のヒント + 規則表",
  human: "人の明示",
};

/**
 * 木の子 task の gate か（celeris ADR-0079 D4 (1)、Phase R2a: `depth` を持つ判定。閾値を深さで上げ、
 * `[execution] gate = "shadow"` でも採用される）。
 */
export function isTreeGate(decision: ExecutionGateDecision | null | undefined): boolean {
  return decision?.depth != null && decision.depth >= 2;
}

/** 1 行の説明: `atomic（規則表、shadow）— atomic/score` のように出す。判定が無ければ `null`。 */
export function gateDecisionLine(decision: ExecutionGateDecision | null | undefined): string | null {
  if (!decision) return null;
  const parts = [GATE_SOURCE_LABEL[decision.source]];
  if (isTreeGate(decision)) {
    parts.push(`木の子: 深さ ${decision.depth}・閾値 ${decision.threshold}、常に採用`);
  }
  if (decision.shadow) parts.push("shadow: 記録だけ");
  return `${decision.mode}（${parts.join("、")}）— ${decision.rule_id}`;
}

/** `execution_hint` の説明（無ければ `null`）。 */
export function executionHintLine(hint: ExecutionHintSpec | null | undefined): string | null {
  if (!hint) return null;
  return hint.explicit ? `人の明示: ${hint.mode}` : `CoS のヒント: ${hint.mode}（+2 点）`;
}

/** 出す操作。 */
export type ExecutionModeAction =
  /** compound に切り替える（次の dispatch が planner run）。 */
  | "compound"
  /** 計画を持つ Task への replan の依頼。 */
  | "replan"
  /** atomic に戻す（計画が無いときだけ）。 */
  | "atomic"
  /** 終端（failed / cancelled）: 計画を作らせてやり直す（retry + execution=compound）。 */
  | "retry_compound";

export interface ExecutionModeControls {
  actions: ExecutionModeAction[];
  /** 操作を出さない / 一部しか出さない理由（人に見せる 1 文）。 */
  note: string | null;
  /** gate が次の dispatch で判定し直す（人の明示を書いた直後で、判定がまだ無い）。 */
  regatePending: boolean;
}

const EDITABLE: readonly Status[] = ["draft", "ready", "blocked"];

/** gate の対象になりうる Task か（kind=execute、routing あり、対話でない）。support-task 等の残りは celeris が 422 を返す。 */
export function isGateCandidate(task: Task): boolean {
  return task.kind === "execute" && task.routing != null && !task.conversation;
}

export function executionModeControls(task: Task, hasPlan: boolean): ExecutionModeControls {
  const hint = task.routing?.execution_hint ?? null;
  const decision = task.routing?.execution ?? null;
  const regatePending = !hasPlan && decision == null && hint?.explicit === true;
  if (!isGateCandidate(task)) {
    return { actions: [], note: "このタスクは Complexity Gate の対象外です（常に直接実行）。", regatePending: false };
  }
  if ((EDITABLE as readonly string[]).includes(task.status)) {
    if (hasPlan) {
      return {
        actions: ["replan"],
        note: "計画があるので、atomic には戻せません（中止してから atomic でやり直してください）。",
        regatePending,
      };
    }
    const actions: ExecutionModeAction[] = [];
    const current = hint?.explicit ? hint.mode : decision?.mode;
    if (current !== "compound" || !hint?.explicit) actions.push("compound");
    if (current !== "atomic" || !hint?.explicit) actions.push("atomic");
    const note =
      task.status === "blocked" ? "blocked の間は動きません。回答などで ready に戻った次の run から効きます。" : null;
    return { actions, note, regatePending };
  }
  if (task.status === "failed" || task.status === "cancelled") {
    return { actions: ["retry_compound"], note: null, regatePending: false };
  }
  if (task.status === "running" || task.status === "reviewing") {
    return {
      actions: [],
      note: "走っている run は止めません。終わるのを待つか、中止してから「計画を作らせてやり直す」を使ってください。",
      regatePending: false,
    };
  }
  return { actions: [], note: null, regatePending: false };
}

export const EXECUTION_MODE_ACTION_LABEL: Record<ExecutionModeAction, string> = {
  compound: "計画を作らせる（compound に切り替え）",
  replan: "計画を見直させる（replan を依頼）",
  atomic: "atomic に戻す",
  retry_compound: "計画を作らせてやり直す",
};

export const EXECUTION_MODE_ACTION_CONFIRM: Record<ExecutionModeAction, string> = {
  compound:
    "このタスクを分解の経路に入れます。次の run は計画（ExecutionPlan）を作る planner run になります。よろしいですか？",
  replan: "今の計画の見直し（replan）を依頼します。次の run は replan の planner run になります。よろしいですか？",
  atomic: "このタスクを atomic（1 つの run で直接実行）に戻します。よろしいですか？",
  retry_compound: "このタスクを複製して、計画を作る経路でやり直します（新しいタスクができます）。よろしいですか？",
};
