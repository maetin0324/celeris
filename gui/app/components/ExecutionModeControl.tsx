import { type MouseEvent, useEffect } from "react";
import { useFetcher, useNavigate } from "react-router";
import type { RetryOutcome, TaskDecomposeOutcome } from "~/celeris/action-types";
import type { ExecutionView, Task } from "~/celeris/types";
import { ErrorFlash, FieldErrors } from "~/components/Flash";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { hintClass, textareaClass } from "~/components/ui/form";
import { Alert } from "~/components/ui/misc";
import {
  EXECUTION_MODE_ACTION_CONFIRM,
  EXECUTION_MODE_ACTION_LABEL,
  type ExecutionModeAction,
  executionHintLine,
  executionModeControls,
  gateDecisionLine,
  isGateCandidate,
} from "~/lib/execution-mode";

/**
 * タスク詳細の「実行の形」（celeris ADR-0072「Phase F6 実装時の決定」）。gate の判定の出どころ（規則表 / CoS の
 * ヒント / 人の明示）を見せ、起票済みのタスクを後から分解の経路（ExecutionPlan）に入れる / atomic に戻す。
 * 終端のタスクは「計画を作らせてやり直す」（`retry` + `execution: "compound"`。新しいタスクへ移る）。
 * どの操作を出すかは `executionModeControls` が決め、押せるかどうかの最終判断は celeris（409 / 422 の文言を出す）。
 */
export function ExecutionModeControl({ task, execution }: { task: Task; execution: ExecutionView | null | undefined }) {
  const fetcher = useFetcher<TaskDecomposeOutcome>({ key: `task-decompose-${task.id}` });
  const retryFetcher = useFetcher<RetryOutcome>({ key: `task-retry-compound-${task.id}` });
  const navigate = useNavigate();
  useEffect(() => {
    if (retryFetcher.data?.ok) navigate(`/tasks/${retryFetcher.data.result.task_id}`);
  }, [retryFetcher.data, navigate]);
  if (!isGateCandidate(task)) return null;

  const hasPlan = execution?.plan != null;
  const controls = executionModeControls(task, hasPlan);
  const decision = task.routing?.execution ?? execution?.gate ?? null;
  const decisionLine = gateDecisionLine(decision);
  const hintLine = executionHintLine(task.routing?.execution_hint);
  const submitting = fetcher.state !== "idle" || retryFetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  const decomposeActions = controls.actions.filter((a) => a !== "retry_compound");
  const confirmSubmit = (action: ExecutionModeAction) => (e: MouseEvent<HTMLButtonElement>) => {
    if (!window.confirm(EXECUTION_MODE_ACTION_CONFIRM[action])) e.preventDefault();
  };

  return (
    <section aria-labelledby="execution-mode-heading" data-testid="execution-mode">
      <Card>
        <CardHeader
          icon="gitBranch"
          title={
            <h2 id="execution-mode-heading" className="text-[0.95rem] font-semibold text-fg">
              実行の形（atomic / compound）
            </h2>
          }
          description="なぜ直接実行（atomic）か・計画（compound）かと、その決め直し。"
        />
        <CardBody className="space-y-3 text-sm">
          <p className="text-fg-muted" data-testid="execution-mode-decision">
            gate の判定:{" "}
            {decisionLine ?? (controls.regatePending ? "次の run で判定し直します" : "まだ判定していません")}
          </p>
          {hintLine && (
            <p className="text-fg-muted" data-testid="execution-mode-hint">
              {hintLine}
            </p>
          )}
          {controls.regatePending && (
            <p className="text-fg-muted" data-testid="execution-mode-regate-pending">
              人の明示を書きました。次の dispatch で gate が human/explicit として判定し直します。
            </p>
          )}
          {controls.note && (
            <p className={hintClass} data-testid="execution-mode-note">
              {controls.note}
            </p>
          )}
          <TaskDecomposeFlash outcome={fetcher.data} />
          {decomposeActions.length > 0 && (
            <fetcher.Form
              method="post"
              action={`/tasks/${task.id}`}
              className="space-y-2"
              data-testid="execution-mode-form"
            >
              <input type="hidden" name="intent" value="execution_decompose" />
              <label className="block">
                <span className="font-medium text-fg">ひとこと（任意。replan では planner に渡ります）</span>
                <textarea name="note" rows={2} className={textareaClass} data-testid="execution-mode-note-input" />
              </label>
              <FieldErrors error={error} field="note" />
              <div className="flex flex-wrap gap-2">
                {decomposeActions.map((action) => (
                  <Button
                    key={action}
                    type="submit"
                    name="mode"
                    value={action === "atomic" ? "atomic" : "compound"}
                    variant={action === "atomic" ? "secondary" : "primary"}
                    size="sm"
                    disabled={submitting}
                    onClick={confirmSubmit(action)}
                    data-testid={`execution-mode-${action}`}
                  >
                    {EXECUTION_MODE_ACTION_LABEL[action]}
                  </Button>
                ))}
              </div>
            </fetcher.Form>
          )}
          {controls.actions.includes("retry_compound") && (
            <retryFetcher.Form method="post" action={`/tasks/${task.id}`} data-testid="execution-mode-retry-form">
              <input type="hidden" name="intent" value="retry" />
              <input type="hidden" name="execution" value="compound" />
              <Button
                type="submit"
                variant="primary"
                size="sm"
                disabled={submitting}
                onClick={confirmSubmit("retry_compound")}
                data-testid="execution-mode-retry_compound"
              >
                {EXECUTION_MODE_ACTION_LABEL.retry_compound}
              </Button>
              {retryFetcher.data && !retryFetcher.data.ok && <ErrorFlash error={retryFetcher.data.error} />}
            </retryFetcher.Form>
          )}
        </CardBody>
      </Card>
    </section>
  );
}

/**
 * celeris ADR-0072「Phase F6 実装時の決定」: 実行の形を決め直した結果（次の dispatch から効く）。
 * F6-fix（ADR-0055 性能予算）: 使うのはこの部品だけなので、タスク詳細の初回チャンクに載る `~/components/Flash` から移した。
 */
export function TaskDecomposeFlash({ outcome }: { outcome: TaskDecomposeOutcome | undefined | null }) {
  if (!outcome) return null;
  if (!outcome.ok) return <ErrorFlash error={outcome.error} />;
  const { result } = outcome;
  return (
    <Alert role="status" data-testid="flash" data-flash-kind="ok" tone="success" className="my-2">
      <p data-testid="flash-task-decompose">
        {result.replan
          ? "計画の見直し（replan）を依頼しました。次の run は replan の planner run です。"
          : result.mode === "compound"
            ? "compound に切り替えました。次の run は計画を作る planner run です。"
            : "atomic に切り替えました。次の run は直接実行です。"}
      </p>
    </Alert>
  );
}
