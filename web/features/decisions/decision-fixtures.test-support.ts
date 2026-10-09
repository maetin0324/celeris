import type { DecisionOutcome, DecisionView } from "../../api/generated/types";

// decision の試験の fixture（人の指摘 2026-10-09 の nonpool-limit を模す）。

export function decisionView(
  over: Partial<DecisionView["decision"]> = {},
  view: Partial<DecisionView> = {},
): DecisionView {
  return {
    task_id: "T1",
    root_id: "T1",
    created_at: "2026-10-09T00:00:00Z",
    answered_at: "2026-10-09T01:00:00Z",
    effect: "resume",
    ...view,
    decision: {
      id: "D1",
      key: "nonpool-limit",
      kind: "choice",
      question: "プール外の上限をどう記録するか",
      options: [
        { key: "explicit", label: "(i) explicit-account", consequence: "明示の account に記録" },
        { key: "implicit", label: "(ii) implicit-account" },
      ],
      recommended: "implicit",
      cost_of_reversal: "low",
      needed_before: [],
      path: [],
      raised_by: { origin: "planner", task_id: "T1" },
      status: "answered",
      answer: { option: "explicit", by: "human", note: "最初の答え" },
      ...over,
    },
  };
}

export const outcome = (view: DecisionView, over: Partial<DecisionOutcome> = {}): DecisionOutcome => ({
  decision: view,
  effect: "resume",
  cancelled: [],
  resumed: [],
  replan_requested: false,
  notified_children: ["C1", "C2"],
  ...over,
});
