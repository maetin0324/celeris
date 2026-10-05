import type {
  ActualSource,
  CandidateTrace,
  RequestRoutingAudit,
  RoutingOutcome,
  RoutingTraceV1,
  RunRoutingAudit,
  TaskRoutingView,
} from "../../api/generated/types";
import { UNKNOWN } from "../ops/routing-catalog";
import { auditIncompleteLabel, costRows, excludedLabel, scoreLabel, usdLabel } from "../ops/routing-state";
import { CostRows } from "../ops/routing-state-view";

// タスクの routing 監査（GET /tasks/:id/routing）。run ごとに、要求単位の決定（候補の除外理由・
// 費用の 4 成分・score 内訳・最終 source/model/account）を出す。監査が不完全なら、その旨を先に言う。
// 選択は daemon が記録した値をそのまま出す。ここで選び直さない。
//
// ADR 2026-10-04-multi-objective-model-routing Phase 3（§5・§6・§10）: 実行 lane の選定理由（reasons）、
// 実際に使った source（actual_sources。dispatch で未確定だった model を含む）、構造化した軌跡
// escalation（escalation_audit）、run の outcome（outcome_state で not_recorded・unreviewed・judged を
// 区別。audit_incomplete とは別の軸で、どちらも false や 0 に丸めない）を追加で出す。

export function RoutingAuditView({ data }: { data: TaskRoutingView }) {
  return (
    <div className="space-y-2 text-sm">
      <p className="break-words">担当 {data.assignee ?? "未定"}</p>
      {data.runs.length === 0 ? <p>routing の記録はありません。</p> : null}
      {data.runs.length > 0 && (
        <ul className="space-y-3">
          {data.runs.map((run) => (
            <RunAudit key={run.run_id} run={run} />
          ))}
        </ul>
      )}
      {data.unbound_requests?.length ? (
        <div className="space-y-2">
          <p className="font-semibold">どの run にも結べない要求</p>
          <RequestList requests={data.unbound_requests} />
        </div>
      ) : null}
    </div>
  );
}

function RunAudit({ run }: { run: RunRoutingAudit }) {
  const incomplete = auditIncompleteLabel(run.audit_incomplete, run.incomplete_reasons);
  return (
    <li className="space-y-1 break-words" data-testid="routing-run">
      <p>
        {run.run_id}: 実行 lane {run.lane ?? UNKNOWN} / 組織 {run.org_node ?? UNKNOWN}
        {run.rule_id ? `（${run.rule_id}）` : ""}
      </p>
      {run.reasons && run.reasons.length > 0 && <p>選定理由: {run.reasons.join(", ")}</p>}
      <p>
        実行枠: 供給元 {run.provider ?? UNKNOWN} / model {run.model ?? UNKNOWN} / 口座 {run.account ?? UNKNOWN}
      </p>
      <p>実測の請求: {usdLabel(run.cost_usd)}</p>
      {incomplete && (
        <p role="status" className="font-semibold">
          {incomplete}
        </p>
      )}
      {run.actual_sources && run.actual_sources.length > 0 ? <ActualSourceList sources={run.actual_sources} /> : null}
      {run.escalation_audit ? <EscalationView escalation={run.escalation_audit} /> : null}
      <OutcomeView state={run.outcome_state} outcome={run.routing_outcome} />
      {run.requests === null || run.requests === undefined ? (
        <p className="text-neutral-700">要求単位の記録がない旧 run です（欄は不明）。</p>
      ) : (
        <RequestList requests={run.requests} />
      )}
      {run.optimizer ? (
        <div className="space-y-1">
          <p className="font-semibold">最適化の比較（mode {run.optimizer.mode}）</p>
          <TraceView trace={run.optimizer} />
        </div>
      ) : null}
    </li>
  );
}

function ActualSourceList({ sources }: { sources: readonly ActualSource[] }) {
  return (
    <div>
      <p className="font-semibold">実際に使った source</p>
      <ul className="ml-4 list-disc">
        {sources.map((source, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: 同じ source/model/account の重複は無い（日記の順）
          <li key={i}>
            source {source.source_id ?? UNKNOWN} / model {source.model ?? UNKNOWN} / 口座 {source.account ?? UNKNOWN}
            （出所: {source.from}）
          </li>
        ))}
      </ul>
    </div>
  );
}

function EscalationView({ escalation }: { escalation: NonNullable<RunRoutingAudit["escalation_audit"]> }) {
  return (
    <p>
      escalation: {escalation.requested_lane}
      {escalation.previous_lane ? `（直前 ${escalation.previous_lane}）` : ""} → {escalation.selected_lane} / 理由{" "}
      {escalation.reason}（連続失敗 {escalation.counted_failures} 回、区間 {escalation.interval_id}）
    </p>
  );
}

function OutcomeView({
  state,
  outcome,
}: {
  state: RunRoutingAudit["outcome_state"];
  outcome: RoutingOutcome | null | undefined;
}) {
  if (!state) return null;
  if (state === "not_recorded") return <p>outcome: 未記録</p>;
  if (state === "unreviewed") return <p>outcome: 未レビュー（合否は null のまま）</p>;
  return (
    <p>
      outcome: 判定済み / 受け入れ {judgedLabel(outcome?.acceptance_passed)} / review{" "}
      {judgedLabel(outcome?.review_passed)} / reward {outcome?.reward ?? UNKNOWN}
    </p>
  );
}

function judgedLabel(value: boolean | null | undefined): string {
  if (value === true) return "合格";
  if (value === false) return "不合格";
  return UNKNOWN;
}

function RequestList({ requests }: { requests: readonly RequestRoutingAudit[] }) {
  if (requests.length === 0) return <p>要求の記録はありません。</p>;
  return (
    <ul className="ml-4 list-disc space-y-2">
      {requests.map((request) => (
        <li key={request.decision_id} className="break-words">
          <p>
            要求 {request.request_id ?? UNKNOWN}（決定 {request.decision_id}）
          </p>
          {request.incomplete_reason && (
            <p role="status" className="font-semibold">
              結べていません: {request.incomplete_reason}
            </p>
          )}
          <TraceView trace={request.trace} />
        </li>
      ))}
    </ul>
  );
}

function TraceView({ trace }: { trace: RoutingTraceV1 }) {
  return (
    <div className="space-y-1">
      <p>
        段 {trace.stage} / 要求の段 {trace.requested_lane} → 選択の段 {trace.selected_lane ?? UNKNOWN} / mode{" "}
        {trace.mode}
      </p>
      <p>
        最終: source {trace.source_id ?? UNKNOWN} / model {trace.model ?? UNKNOWN} / 口座 {trace.account_id ?? UNKNOWN}
      </p>
      <p>選択: {trace.selected ?? "なし"}</p>
      {trace.reasons.length > 0 && <p>理由: {trace.reasons.join(", ")}</p>}
      {trace.fallback_order.length > 0 && <p>fallback の順: {trace.fallback_order.join(" → ")}</p>}
      {trace.candidates.length === 0 ? (
        <p>候補の記録はありません。</p>
      ) : (
        <ul className="space-y-2">
          {trace.candidates.map((candidate) => (
            <CandidateRow key={candidate.deployment_id} candidate={candidate} selected={trace.selected} />
          ))}
        </ul>
      )}
    </div>
  );
}

function CandidateRow({ candidate, selected }: { candidate: CandidateTrace; selected?: string | null }) {
  const status =
    selected === candidate.deployment_id
      ? "選択"
      : candidate.excluded_reason || candidate.excluded_reasons.length > 0
        ? excludedLabel(candidate.excluded_reason, candidate.excluded_reasons)
        : selected
          ? "除外なし（選ばれず）"
          : "除外なし";
  return (
    <li className="break-words" aria-label={`候補 ${candidate.deployment_id}`}>
      <p className="font-semibold">
        {candidate.deployment_id}（{candidate.model_profile_id}）: {status}
      </p>
      <p>
        score {candidate.score ?? UNKNOWN} / 遅延{" "}
        {candidate.latency_ms == null ? UNKNOWN : `${candidate.latency_ms} ms`}
        {" / "}品質 {candidate.quality?.index ?? UNKNOWN}
        {candidate.quality?.confidence == null ? "" : `（信頼度 ${candidate.quality.confidence}）`}
      </p>
      <p>{scoreLabel(candidate.score_breakdown, candidate.score)}</p>
      <CostRows
        rows={costRows({
          cash: candidate.cash_usd,
          shadow: candidate.shadow_usd,
          resource: candidate.resource_usd,
          effective: candidate.effective_usd,
        })}
      />
    </li>
  );
}
