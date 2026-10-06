import type { CandidateTrace, LlmSourceStateView, RequestRoutingAudit } from "~/celeris/types";
import { Mono } from "~/components/ui/misc";
import { shortId } from "~/lib/format";
import {
  actualSourceLine,
  candidateStatusLabel,
  costRows,
  deploymentSummary,
  finalSelection,
  incompleteReasonLabel,
  scoreRows,
  UNKNOWN_LABEL,
  usdLabel,
} from "~/lib/routing-source-state";

/**
 * 多目的 routing の表示部品（ADR 2026-10-04-multi-objective-model-routing Phase 2）。
 * `/accounts` の LLM source 節（deployment ごとの鮮度・残量・負荷・費用）と、タスク詳細の routing 監査
 * （要求ごとの候補・除外理由・score 内訳・最終の source/model/account）で共有する。
 * 値の判断は celeris の側で済んでいる。ここでは並べて「不明」を明示するだけ。
 */

const dtClass = "text-sm font-medium text-fg-subtle lg:text-xs";
const ddClass = "min-w-0 break-words text-sm text-fg";

/** 請求と機会費用を別の欄で出す（未知は「不明」。0 円に見せない）。 */
function CostList({ cost }: { cost: LlmSourceStateView["cost"] }) {
  return (
    <dl className="space-y-1" data-testid="routing-cost">
      {costRows(cost).map((row) => (
        <div key={row.key} className="flex min-w-0 items-baseline justify-between gap-3" data-cost-key={row.key}>
          <dt
            className={
              row.opportunity ? "min-w-0 text-sm text-fg-subtle lg:text-xs" : "min-w-0 text-sm text-fg-muted lg:text-xs"
            }
          >
            {row.label}
          </dt>
          <dd
            className={`shrink-0 text-right text-sm tabular-nums ${row.value === UNKNOWN_LABEL ? "text-fg-subtle" : "text-fg"}`}
            data-testid={`routing-cost-${row.key}`}
          >
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** `LlmSourceView.deployments`（deployment ごとの動的状態）。空なら何も出さない。 */
export function SourceDeployments({ deployments, nowMs }: { deployments: LlmSourceStateView[]; nowMs: number }) {
  if (deployments.length === 0) return null;
  return (
    <ul className="space-y-3" data-testid="routing-deployments">
      {deployments.map((state) => {
        const s = deploymentSummary(state, nowMs);
        return (
          <li
            key={state.deployment_id}
            className="min-w-0 space-y-2 rounded-lg border border-border bg-surface-2 p-3"
            data-testid="routing-deployment"
            data-deployment-id={state.deployment_id}
          >
            <Mono className="break-all text-xs text-fg-muted">{state.deployment_id}</Mono>
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5">
              <dt className={dtClass}>鮮度</dt>
              <dd className={ddClass} data-testid="routing-freshness">
                <span className={s.freshness.stale ? "text-warning-soft-fg" : undefined}>{s.freshness.text}</span>
              </dd>
              <dt className={dtClass}>到達</dt>
              <dd
                className={`${ddClass} ${state.reachability === "down" ? "text-danger-soft-fg" : ""}`}
                data-testid="routing-reachability"
              >
                {s.reachability}
              </dd>
              <dt className={dtClass}>遅延</dt>
              <dd className={ddClass}>{s.latency}</dd>
              <dt className={dtClass}>残量</dt>
              <dd className={ddClass} data-testid="routing-quota">
                {s.quota}
                <span className="ml-1.5 text-fg-subtle">（reset {s.quotaReset}）</span>
              </dd>
              <dt className={dtClass}>負荷</dt>
              <dd className={ddClass}>{s.pressure}</dd>
            </dl>
            <CostList cost={state.cost} />
            {s.unknown.length > 0 && (
              <p className="text-sm text-fg-subtle" data-testid="routing-unknown">
                不明: {s.unknown.join("・")}
              </p>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** 1 候補の行: 状態（除外理由）・費用の成分・score の内訳。 */
function CandidateItem({ candidate }: { candidate: CandidateTrace }) {
  const status = candidateStatusLabel(candidate);
  const rows = scoreRows(candidate.score_breakdown);
  return (
    <li
      className="min-w-0 space-y-1.5 rounded-lg border border-border p-2.5"
      data-testid="routing-candidate"
      data-deployment-id={candidate.deployment_id}
    >
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
        <Mono className="min-w-0 break-all text-xs text-fg">{candidate.deployment_id}</Mono>
        <span
          className={`text-sm ${status.excluded ? "text-danger-soft-fg" : "text-fg"}`}
          data-testid="routing-candidate-status"
        >
          {status.label}
        </span>
      </div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-sm">
        <dt className={dtClass}>請求</dt>
        <dd className={ddClass}>{usdLabel(candidate.cash_usd)}</dd>
        <dt className={dtClass}>機会（残量）</dt>
        <dd className={ddClass}>{usdLabel(candidate.shadow_usd)}</dd>
        <dt className={dtClass}>機会（資源）</dt>
        <dd className={ddClass}>{usdLabel(candidate.resource_usd)}</dd>
        <dt className={dtClass}>score</dt>
        <dd className={ddClass} data-testid="routing-candidate-score">
          {candidate.score == null ? UNKNOWN_LABEL : candidate.score.toFixed(3)}
        </dd>
      </dl>
      {rows.length > 0 && (
        // ADR-0055 D1-6: 表は overflow-x-auto の箱に入れる（狭い画面で列がはみ出しても箱の中で横に送る）。
        <div className="overflow-x-auto">
          <table className="w-full table-fixed text-xs" data-testid="routing-score-breakdown">
            <tbody>
              {rows.map((r) => (
                <tr key={r.key} className="border-t border-border">
                  <th scope="row" className="py-0.5 pr-2 text-left font-normal text-fg-muted">
                    {r.label}
                    {r.unknownNote && <span className="ml-1 text-warning-soft-fg">{r.unknownNote}</span>}
                  </th>
                  <td className="w-16 py-0.5 text-right tabular-nums text-fg">{r.value.toFixed(3)}</td>
                  <td className="w-14 py-0.5 text-right tabular-nums text-fg-subtle">×{r.weight.toFixed(2)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </li>
  );
}

/** 要求 1 件の監査: 最終の source・model・account と、候補ごとの除外理由・score。 */
export function RequestAudit({ request }: { request: RequestRoutingAudit }) {
  const final = finalSelection(request);
  const candidates = request.trace.candidates;
  return (
    <div className="min-w-0 space-y-2" data-testid="routing-request" data-decision-id={request.decision_id}>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
        <dt className={dtClass}>要求</dt>
        <dd className={ddClass}>
          <Mono title={request.decision_id}>{shortId(request.decision_id)}</Mono>
        </dd>
        <dt className={dtClass}>最終</dt>
        <dd className={ddClass} data-testid="routing-final">
          <span data-final-field="source">source {final.source}</span>
          <span className="mx-1.5 text-fg-subtle">/</span>
          <span data-final-field="model">model {final.model}</span>
          <span className="mx-1.5 text-fg-subtle">/</span>
          <span data-final-field="account">account {final.account}</span>
        </dd>
        {(request.trace.reasons ?? []).length > 0 && (
          <>
            <dt className={dtClass}>選択理由</dt>
            <dd className={ddClass} data-testid="routing-request-reasons">
              {(request.trace.reasons ?? []).join("・")}
            </dd>
          </>
        )}
      </dl>
      {request.incomplete_reason && (
        <p className="text-sm text-warning-soft-fg" data-testid="routing-request-incomplete">
          {incompleteReasonLabel(request.incomplete_reason)}
        </p>
      )}
      {(request.attempts ?? []).length > 0 && (
        <p className="text-sm text-fg" data-testid="routing-request-attempts">
          試した順: {(request.attempts ?? []).map((a) => a.source_id).join(" → ")}
          {request.fallback_reason && (
            <span className="ml-1.5 text-warning-soft-fg" data-testid="routing-request-fallback">
              （最後の source へ落ちた原因: {request.fallback_reason}）
            </span>
          )}
        </p>
      )}
      {request.fallback_reason && (request.attempts ?? []).length === 0 && (
        <p className="text-sm text-warning-soft-fg" data-testid="routing-request-fallback">
          最後の source へ落ちた原因: {request.fallback_reason}
        </p>
      )}
      {request.actual && (
        <p className="text-sm text-fg" data-testid="routing-request-actual">
          実際: {actualSourceLine(request.actual)}
        </p>
      )}
      {candidates.length > 0 && (
        <ul className="space-y-2" data-testid="routing-candidates">
          {candidates.map((c) => (
            <CandidateItem key={`${c.config_order ?? ""}:${c.deployment_id}`} candidate={c} />
          ))}
        </ul>
      )}
    </div>
  );
}
