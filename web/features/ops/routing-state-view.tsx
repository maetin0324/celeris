import type { LlmSourceStateView } from "../../api/generated/types";
import { costRows, sourceStateLines } from "./routing-state";

/** 費用の行（請求・機会費用・実効を別の行で）。 */
export function CostRows({ rows }: { rows: ReturnType<typeof costRows> }) {
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 text-sm">
      {rows.map((row) => (
        <div key={row.label} className="contents">
          <dt className="text-neutral-700">{row.label}</dt>
          <dd className="break-words">{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** source の 1 deployment の状態（到達・鮮度・残量・圧力・遅延と、費用の 4 成分）。 */
export function DeploymentStateList({ states }: { states: readonly LlmSourceStateView[] }) {
  if (states.length === 0) return null;
  return (
    <ul className="space-y-2" aria-label="deployment の状態">
      {states.map((state) => (
        <li
          key={state.deployment_id}
          className="break-words text-sm"
          aria-label={`deployment state ${state.deployment_id}`}
        >
          <span className="font-semibold">{state.deployment_id}</span>
          <ul className="ml-4 list-disc">
            {sourceStateLines(state).map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          <CostRows
            rows={costRows({
              cash: state.cost?.billed.cash_usd,
              shadow: state.cost?.opportunity.shadow_usd,
              resource: state.cost?.opportunity.resource_usd,
              effective: state.cost?.effective_usd,
            })}
          />
          {state.cost?.assumptions?.length ? (
            <p className="text-neutral-700">前提: {state.cost.assumptions.join("、")}</p>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
