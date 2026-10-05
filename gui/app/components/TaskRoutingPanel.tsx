import type { TaskRoutingView } from "~/celeris/types";
import { RequestAudit } from "~/components/RoutingSourceState";
import { Mono } from "~/components/ui/misc";
import { shortId } from "~/lib/format";
import { tierLabel } from "~/lib/labels";
import { actualSourceLine, auditIncompleteNote } from "~/lib/routing-source-state";
import {
  droppedAssigneeNote,
  escalationHistory,
  executedLaneNote,
  featureRows,
  formatCostUsd,
  formatTokens,
  formatWallMs,
  latestRoutingRun,
  outcomeLine,
  outcomeStateLabel,
  ROUTING_PANEL_LABEL,
  reviewResultLabel,
  routingSummaryLine,
  tierSourceLabel,
} from "~/lib/task-routing";

/**
 * 「なぜこの担当・harness・lane・model か」（celeris ADR-0069 D5、`GET /tasks/{id}/routing`）。
 * 閉じた状態は 1 行（`org / harness / lane / model`）、開くと features・当たった規則と policy の版・
 * 理由・コスト等・レビュー結果を出す。値は celeris の監査記録をそのまま並べるだけ（GUI では再計算しない）。
 * 多目的 routing（Phase 2）の要求ごとの決定（候補・除外理由・score・最終の source/model/account）と、
 * 監査が不完全なときの注記も出す。run が無く、捨てた担当も無いタスクでは何も出さない。
 */
export function TaskRoutingPanel({ view }: { view: TaskRoutingView | null }) {
  const run = latestRoutingRun(view);
  const dropped = droppedAssigneeNote(view);
  if (!view || (!run && !dropped)) return null;
  const escalations = escalationHistory(view);
  const laneNote = run ? executedLaneNote(run) : null;
  const dtClass = "text-sm font-medium text-fg-subtle lg:text-xs";
  const ddClass = "min-w-0 break-words text-sm text-fg";
  const ddBreakAll = "min-w-0 break-all text-sm text-fg";
  return (
    <details data-testid="task-routing" className="max-w-full min-w-0 text-sm text-fg-muted">
      <summary className="flex min-h-11 cursor-pointer select-none flex-wrap items-center gap-x-2 gap-y-0.5 lg:min-h-0">
        <span className="font-medium text-fg">{ROUTING_PANEL_LABEL}:</span>
        <span data-testid="task-routing-summary" className="min-w-0 break-all font-mono text-[0.85em]">
          {run ? routingSummaryLine(run) : "まだ run がありません"}
        </span>
        {dropped && (
          <span className="text-sm text-warning-soft-fg lg:text-xs" aria-hidden="true">
            ※
          </span>
        )}
      </summary>
      <div className="mt-2 space-y-3 rounded-lg border border-border bg-surface-2 p-3" data-testid="task-routing-body">
        {dropped && (
          <p data-testid="task-routing-dropped" className="break-words text-sm text-fg">
            {dropped}
          </p>
        )}
        {run && (
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5">
            <dt className={dtClass}>run</dt>
            <dd className={ddClass}>
              <Mono title={run.run_id}>{shortId(run.run_id)}</Mono>
              {view.runs.length > 1 && <span className="ml-1.5 text-fg-subtle">（{view.runs.length} 件中の最新）</span>}
            </dd>
            <dt className={dtClass}>lane</dt>
            <dd className={ddClass} data-testid="task-routing-lane">
              {run.lane ? tierLabel(run.lane) : "—"}
              <span className="ml-1.5 text-fg-subtle">（{tierSourceLabel(view.routing?.tier_source)}）</span>
              {laneNote && (
                <span className="ml-1.5 text-warning-soft-fg" data-testid="task-routing-lane-note">
                  {laneNote}
                </span>
              )}
            </dd>
            <dt className={dtClass}>model</dt>
            <dd className={ddBreakAll}>
              {[run.provider, run.model, run.reasoning_effort].filter(Boolean).join(" · ") || "—"}
            </dd>
            {(run.actual_sources ?? []).length > 0 && (
              <>
                <dt className={dtClass}>実 source</dt>
                <dd className={ddBreakAll} data-testid="task-routing-actual-sources">
                  {(run.actual_sources ?? []).map((a) => actualSourceLine(a)).join("；")}
                </dd>
              </>
            )}
            <dt className={dtClass}>規則</dt>
            <dd className={ddBreakAll} data-testid="task-routing-rule">
              {run.rule_id ?? "—"}
              {run.policy_version && <span className="ml-1.5 text-fg-subtle">（policy {run.policy_version}）</span>}
            </dd>
            <dt className={dtClass}>コスト</dt>
            <dd className={ddClass}>{formatCostUsd(run.cost_usd)}</dd>
            <dt className={dtClass}>トークン</dt>
            <dd className={ddClass}>{formatTokens(run.input_tokens, run.output_tokens)}</dd>
            <dt className={dtClass}>所要</dt>
            <dd className={ddClass}>
              {formatWallMs(run.wall_ms)}
              {run.retries != null && <span className="ml-1.5 text-fg-subtle">（リトライ {run.retries} 回）</span>}
            </dd>
            <dt className={dtClass}>レビュー</dt>
            <dd className={ddClass} data-testid="task-routing-review">
              {reviewResultLabel(run.review)}
            </dd>
            <dt className={dtClass}>outcome</dt>
            <dd className={ddClass} data-testid="task-routing-outcome">
              {outcomeStateLabel(run)}
              {outcomeLine(run.routing_outcome) && (
                <span className="ml-1.5 text-fg" data-testid="task-routing-outcome-line">
                  {outcomeLine(run.routing_outcome)}
                </span>
              )}
            </dd>
          </dl>
        )}
        {run && (run.reasons ?? []).length > 0 && (
          <div>
            <p className={dtClass}>理由</p>
            <ul className="mt-1 list-disc space-y-0.5 pl-5 text-sm text-fg" data-testid="task-routing-reasons">
              {(run.reasons ?? []).map((r) => (
                <li key={r} className="break-words">
                  {r}
                </li>
              ))}
            </ul>
          </div>
        )}
        {run && auditIncompleteNote(run.audit_incomplete, run.incomplete_reasons) && (
          <p data-testid="task-routing-incomplete" className="break-words text-sm text-warning-soft-fg">
            {auditIncompleteNote(run.audit_incomplete, run.incomplete_reasons)}
          </p>
        )}
        {run && (run.requests ?? []).length > 0 && (
          <div className="space-y-2" data-testid="task-routing-requests">
            <p className={dtClass}>要求ごとの決定（{run.requests?.length} 件）</p>
            {(run.requests ?? []).map((request) => (
              <RequestAudit key={request.decision_id} request={request} />
            ))}
          </div>
        )}
        {(view.unbound_requests ?? []).length > 0 && (
          <p className="text-sm text-warning-soft-fg" data-testid="task-routing-unbound">
            どの run にも結べない要求が {view.unbound_requests?.length} 件あります
          </p>
        )}
        {run?.features && (
          <table className="w-full table-fixed text-sm" data-testid="task-routing-features">
            <caption className={`${dtClass} pb-1 text-left`}>features</caption>
            <tbody>
              {featureRows(run.features).map((row) => (
                <tr key={row.key} className="border-t border-border">
                  <th scope="row" className="py-1 pr-2 text-left font-normal text-fg-muted">
                    {row.label}
                  </th>
                  <td className="w-12 py-1 text-right text-fg">{row.level}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {escalations.length > 0 && (
          <div>
            <p className={dtClass}>エスカレーション</p>
            <ul className="mt-1 space-y-0.5 text-sm text-fg" data-testid="task-routing-escalations">
              {escalations.map((e) => (
                <li key={e.runId} className="break-words">
                  <Mono title={e.runId}>{shortId(e.runId)}</Mono> {e.text}
                </li>
              ))}
            </ul>
          </div>
        )}
        {run?.routing_outcome?.supersedes && (
          <p className="text-sm text-fg-subtle" data-testid="task-routing-outcome-supersede">
            前の outcome（{shortId(run.routing_outcome.supersedes)}）を置き換えています
          </p>
        )}
      </div>
    </details>
  );
}
