import { Link, useFetcher } from "react-router";
import type { DecisionActionOutcome, TaskPlanGateOutcome } from "~/celeris/action-types";
import type { Action, AttentionItem, DecisionInboxItem } from "~/celeris/types";
import { ErrorFlash, FieldErrors } from "~/components/Flash";
import { LocalTime } from "~/components/LocalTime";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { chipLabelClass, hintClass, textareaClass } from "~/components/ui/form";
import { Alert } from "~/components/ui/misc";
import {
  COST_OF_REVERSAL_LABEL,
  COST_OF_REVERSAL_TONE,
  DECISION_KIND_LABEL,
  decisionAgeText,
  decisionBreadcrumb,
  decisionChoices,
  neededBeforeText,
  PLAN_GATE_ACTION_LABEL,
  planApprovalReasonLines,
  planGateActions,
} from "~/lib/tree";
import { cn } from "~/lib/utils";

/**
 * celeris ADR-0079 D14（Phase R4b）: 受信箱の「決定」と「計画の承認」、タスク詳細の「実行の形」カードの承認の操作。
 * どの操作を出すかは `~/lib/tree.ts`、押せるかどうかの最終判断は celeris（409 / 422 の文言をそのまま出す）。
 * タスク詳細からは `React.lazy` の先（`ExecutionModeControl`）でしか読まない（ADR-0055 の性能予算）。
 */

/** 回答・取り下げの結果。 */
export function DecisionFlash({ outcome }: { outcome: DecisionActionOutcome | undefined | null }) {
  if (!outcome) return null;
  if (!outcome.ok) return <ErrorFlash error={outcome.error} />;
  const { result } = outcome;
  const resumed = result.resumed.length > 0 ? `再開した unit: ${result.resumed.join("、")}。` : "";
  const cancelled = result.cancelled.length > 0 ? `取り下げた unit: ${result.cancelled.join("、")}。` : "";
  const replan = result.replan_requested ? "計画の立て直し（replan）を依頼しました。" : "";
  return (
    <Alert role="status" data-testid="flash" data-flash-kind="ok" tone="success" className="my-2">
      <p data-testid="flash-decision">
        {outcome.op === "decision_answer" ? "決定に答えました。" : "決定を取り下げました。"}
        {resumed}
        {cancelled}
        {replan}
      </p>
    </Alert>
  );
}

/** 計画の承認への応答の結果。 */
export function PlanGateFlash({ outcome }: { outcome: TaskPlanGateOutcome | undefined | null }) {
  if (!outcome) return null;
  if (!outcome.ok) return <ErrorFlash error={outcome.error} />;
  const { result } = outcome;
  return (
    <Alert role="status" data-testid="flash" data-flash-kind="ok" tone="success" className="my-2">
      <p data-testid="flash-plan-gate">
        計画の承認に応えました: <span data-testid="flash-from">{result.from}</span> →{" "}
        <span data-testid="flash-to">{result.to}</span>（reason: {result.reason}）
      </p>
    </Alert>
  );
}

/**
 * 受信箱の「決定」の 1 件: パンくず・問い・選択肢（推奨に印）・後戻りの大きさ・待っているもの・経過時間と、
 * その場で答える（note 任意）/ 取り下げる。
 */
export function DecisionItemCard({ item, fetchedAt }: { item: DecisionInboxItem; fetchedAt?: string }) {
  const fetcher = useFetcher<DecisionActionOutcome>({ key: `inbox-decision-${item.id}` });
  const submitting = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  const choices = decisionChoices(item);
  const recommended = choices.find((c) => c.recommended)?.key;
  return (
    <li
      id={`decision-${item.id}`}
      data-testid="decision-item"
      data-decision-id={item.id}
      className="space-y-3 rounded-lg border border-warning-border bg-surface p-4 text-sm shadow-xs"
    >
      <p className="break-words text-fg-muted" data-testid="decision-path">
        <Link
          to={`/tasks/${item.task_id}?tab=tree`}
          className="inline-flex min-h-11 items-center font-medium text-primary hover:underline"
        >
          {decisionBreadcrumb(item.path) || item.task_id}
        </Link>
      </p>
      <p className="break-words font-semibold text-fg" data-testid="decision-question">
        {item.question}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="neutral" data-testid="decision-kind">
          {DECISION_KIND_LABEL[item.kind]}
        </Badge>
        <Badge tone={COST_OF_REVERSAL_TONE[item.cost_of_reversal]} data-testid="decision-cost">
          後戻り: {COST_OF_REVERSAL_LABEL[item.cost_of_reversal]}
        </Badge>
        <span className="text-fg-subtle" data-testid="decision-age">
          {decisionAgeText(item.age_secs)}
          {"（"}
          <LocalTime iso={item.created_at} fetchedAtIso={fetchedAt} mode="datetime" />
          {"）"}
        </span>
      </div>
      {item.cost_note && <p className="break-words text-fg-muted">{item.cost_note}</p>}
      <p className="break-words text-fg-muted" data-testid="decision-needed-before">
        待っているもの: {neededBeforeText(item.needed_before)}
      </p>
      <fetcher.Form method="post" action="/inbox" className="space-y-2" data-testid="decision-form">
        <input type="hidden" name="decision_id" value={item.id} />
        <fieldset className="space-y-1.5">
          <legend className="font-medium text-fg">答え</legend>
          {choices.map((c) => (
            <label key={c.key || "free"} className={cn(chipLabelClass, "w-full")} data-testid="decision-option">
              <input
                type="radio"
                name="option"
                value={c.key}
                defaultChecked={c.key === recommended}
                className="shrink-0"
                data-testid={`decision-option-${c.key || "free"}`}
              />
              <span className="min-w-0 break-words">
                {c.label}
                {c.recommended && (
                  <span className="ml-1.5 font-semibold" data-testid="decision-recommended">
                    （推奨）
                  </span>
                )}
                {c.consequence && <span className="block text-fg-subtle">{c.consequence}</span>}
              </span>
            </label>
          ))}
        </fieldset>
        <FieldErrors error={error} field="option" />
        <label className="block">
          <span className="font-medium text-fg">ひとこと（任意。自由記述のときは答えそのもの）</span>
          <textarea name="note" rows={2} className={textareaClass} data-testid="decision-note" />
        </label>
        <FieldErrors error={error} field="note" />
        <div className="flex flex-wrap gap-2">
          <Button
            type="submit"
            name="intent"
            value="decision_answer"
            variant="primary"
            size="sm"
            disabled={submitting}
            data-testid="decision-answer"
          >
            答える
          </Button>
          <Button
            type="submit"
            name="intent"
            value="decision_withdraw"
            variant="danger"
            size="sm"
            disabled={submitting}
            data-testid="decision-withdraw"
          >
            取り下げる
          </Button>
        </div>
        <p className={hintClass}>
          取り下げると、この決定を待っていた unit（とそれに依存する未着手の unit）を行わずに進めます。
        </p>
      </fetcher.Form>
      <DecisionFlash outcome={fetcher.data} />
    </li>
  );
}

/** 計画の承認の 3 つのボタンと指示の欄（受信箱とタスク詳細で共有。送り先は `/tasks/:id` の action）。 */
export function PlanGateForm({ taskId, actions }: { taskId: string; actions: readonly Action[] }) {
  const fetcher = useFetcher<TaskPlanGateOutcome>({ key: `task-plan-gate-${taskId}` });
  const submitting = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  const buttons = planGateActions(actions);
  if (buttons.length === 0) return null;
  return (
    <div className="space-y-2">
      <fetcher.Form method="post" action={`/tasks/${taskId}`} className="space-y-2" data-testid="plan-gate-form">
        <input type="hidden" name="intent" value="plan_gate" />
        <label className="block">
          <span className="font-medium text-fg">人の指示（立て直すときは必須、承認のときは任意）</span>
          <textarea name="note" rows={2} className={textareaClass} data-testid="plan-gate-note" />
        </label>
        <FieldErrors error={error} field="note" />
        <div className="flex flex-wrap gap-2">
          {buttons.map((action) => (
            <Button
              key={action}
              type="submit"
              name="plan_action"
              value={action}
              variant={action === "approve" ? "primary" : action === "replan" ? "secondary" : "danger"}
              size="sm"
              disabled={submitting}
              data-testid={`plan-gate-${action}`}
            >
              {PLAN_GATE_ACTION_LABEL[action]}
            </Button>
          ))}
        </div>
        <p className={hintClass}>
          承認は決定への回答とは別です。答えの無い決定を待つ unit は、承認の後も答えるまで待ちます。
        </p>
      </fetcher.Form>
      <PlanGateFlash outcome={fetcher.data} />
    </div>
  );
}

/** 受信箱の「計画の承認」の 1 件（`attention[]` の `plan_approval`）: 理由・計画の見取り図・同じ節点の決定と 3 つのボタン。 */
export function PlanApprovalCard({ item }: { item: Extract<AttentionItem, { type: "plan_approval" }> }) {
  return (
    <li
      data-testid="plan-approval-item"
      data-attention-type={item.type}
      className="space-y-3 rounded-lg border border-warning-border bg-surface p-4 text-sm shadow-xs"
    >
      <p className="font-semibold text-fg">
        <Link to={`/tasks/${item.task.id}`} className="flex min-h-11 items-center hover:underline">
          {item.task.title}
        </Link>
      </p>
      <p className="break-words text-fg" data-testid="plan-approval-summary">
        計画 v{item.plan_version} の承認を待っています: {item.summary}
      </p>
      <ul className="list-disc space-y-0.5 pl-5 text-fg-muted" data-testid="plan-approval-reasons">
        {planApprovalReasonLines(item.reasons).map((line) => (
          <li key={line} className="break-words">
            {line}
          </li>
        ))}
      </ul>
      <ol className="space-y-2" data-testid="plan-approval-stages">
        {item.stages.map((stage) => (
          <li key={stage.key} className="rounded-md border border-border bg-surface-2/40 p-2">
            <p className="break-words font-medium text-fg">
              {stage.title}
              {stage.review_human && <span className="ml-1.5 text-fg-subtle">（段階の後で人の確認）</span>}
            </p>
            <ul className="mt-1 space-y-0.5 break-words text-fg-muted">
              {stage.units.map((u) => (
                <li key={u}>{u}</li>
              ))}
            </ul>
          </li>
        ))}
      </ol>
      {item.decision_ids.length > 0 && (
        <p className="text-fg-muted" data-testid="plan-approval-decisions">
          この計画には未回答の決定が {item.decision_ids.length} 件あります（下の「決定」で答えられます）:{" "}
          {item.decision_ids.map((id) => (
            <a
              key={id}
              href={`#decision-${id}`}
              className="mr-2 inline-flex min-h-11 items-center font-mono text-primary hover:underline"
            >
              {id.slice(-8)}
            </a>
          ))}
        </p>
      )}
      <PlanGateForm taskId={item.task.id} actions={item.task.actions} />
    </li>
  );
}
