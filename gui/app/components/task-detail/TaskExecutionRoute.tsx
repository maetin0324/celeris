import type { ExecutionView } from "~/celeris/types";
import { Badge } from "~/components/ui/badge";

/** Shows the deterministic planner/direct choice and its recorded rule explanations. */
export function TaskExecutionRoute({ execution }: { execution: ExecutionView | null | undefined }) {
  const decision = execution?.route;
  if (!decision) return null;
  const direct = decision.route === "direct";
  const reasons = decision.reasons ?? [];
  return (
    <div className="space-y-1 text-sm text-fg-muted" data-testid="task-execution-route-panel">
      <Badge tone={direct ? "success" : "neutral"} data-testid="task-execution-route">
        {direct ? "直行" : "計画経路"}
      </Badge>
      <p data-testid="task-execution-route-explanation">
        {direct
          ? "判定条件を満たしたため、planner を介さず実装へ進みます。"
          : "判定条件を満たさなかったため、planner による計画経路で進みます。"}
      </p>
      {reasons.length > 0 && (
        <ul className="list-disc space-y-0.5 pl-5" data-testid="task-execution-route-reasons">
          {reasons.map((reason) => (
            <li key={reason.rule_id}>
              <span className="font-mono">{reason.rule_id}</span>: {reason.detail}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
