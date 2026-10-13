import type {
  RequestRoutingAudit,
  RoutingOutcome,
  RoutingShadowAudit,
  RunRoutingAudit,
  TaskRoutingView,
} from "../../api/generated/types";

const UNKNOWN = "不明";
const usdLabel = (value: number | null | undefined) => (value == null ? UNKNOWN : `$${value}`);

// タスクの routing 監査（GET /tasks/:id/routing）。run ごとに、実行した枠（harness・source・model・口座）、
// 選定理由、escalation、run の outcome（outcome_state で not_recorded・unreviewed・judged を区別。
// 未判定を false や 0 に丸めない）を出す。shadow 監査は primary の欄とは別の節に出す。
// 選択は daemon が記録した値をそのまま出す。ここで選び直さない。欠測は「不明」と書く。

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
  return (
    <li className="space-y-1 break-words" data-testid="routing-run">
      <p>
        {run.run_id}: 実行 lane {run.lane ?? UNKNOWN} / 組織 {run.org_node ?? UNKNOWN}
        {run.rule_id ? `（${run.rule_id}）` : ""}
      </p>
      {run.reasons && run.reasons.length > 0 && <p>選定理由: {run.reasons.join(", ")}</p>}
      <p>
        実行枠: harness {run.harness ?? UNKNOWN} / source {run.provider ?? UNKNOWN} / model {run.model ?? UNKNOWN} /
        口座 {run.account ?? UNKNOWN}
      </p>
      <p>実測の請求: {usdLabel(run.cost_usd)}</p>
      {run.escalation_audit ? <EscalationView escalation={run.escalation_audit} /> : null}
      <OutcomeView state={run.outcome_state} outcome={run.routing_outcome} />
      {run.routing_shadow && run.routing_shadow.length > 0 ? <ShadowList shadows={run.routing_shadow} /> : null}
    </li>
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

const SHADOW_KIND_LABEL: Record<RoutingShadowAudit["kind"], string> = {
  decision: "判断のみ（decision、追加呼出しなし）",
  execution: "実行（execution、候補で生成）",
  estimator: "品質推定（estimator、primary の選択には効かない）",
};

const SHADOW_STATUS_LABEL: Record<RoutingShadowAudit["status"], string> = {
  completed: "完了",
  failed: "失敗",
  dropped: "破棄",
};

function ShadowList({ shadows }: { shadows: readonly RoutingShadowAudit[] }) {
  return (
    <section className="space-y-1" aria-label="shadow 監査">
      <p className="font-semibold">shadow（primary の選択は変えない比較）</p>
      <ul className="ml-4 list-disc space-y-2">
        {shadows.map((shadow) => (
          <ShadowRow key={shadow.shadow_id} shadow={shadow} />
        ))}
      </ul>
    </section>
  );
}

function ShadowRow({ shadow }: { shadow: RoutingShadowAudit }) {
  return (
    <li className="break-words" aria-label={`shadow ${shadow.shadow_id}`}>
      <p>
        shadow {shadow.shadow_id}（primary の決定 {shadow.primary_decision_id}）: {SHADOW_KIND_LABEL[shadow.kind]} /{" "}
        {SHADOW_STATUS_LABEL[shadow.status]}
        {shadow.status === "completed" ? "" : ` / 理由 ${shadow.reason ?? UNKNOWN}`}
      </p>
      <p>
        候補: source {shadow.candidate_source ?? UNKNOWN} / model {shadow.candidate_model ?? UNKNOWN} / primary との差{" "}
        {differsLabel(shadow.differs_from_primary)}
      </p>
      {shadow.kind === "execution" ? (
        <p>
          token: 入力 {shadow.input_tokens ?? UNKNOWN} / 出力 {shadow.output_tokens ?? UNKNOWN}
        </p>
      ) : null}
    </li>
  );
}

function differsLabel(value: boolean | null | undefined): string {
  if (value === true) return "あり";
  if (value === false) return "なし（同じ）";
  return UNKNOWN;
}

function judgedLabel(value: boolean | null | undefined): string {
  if (value === true) return "合格";
  if (value === false) return "不合格";
  return UNKNOWN;
}

function RequestList({ requests }: { requests: readonly RequestRoutingAudit[] }) {
  return (
    <ul className="ml-4 list-disc space-y-2">
      {requests.map((request) => (
        <li key={request.decision_id} className="break-words">
          <p>
            要求 {request.request_id ?? UNKNOWN}（決定 {request.decision_id}）
          </p>
        </li>
      ))}
    </ul>
  );
}
