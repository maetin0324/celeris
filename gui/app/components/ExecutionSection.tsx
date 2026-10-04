import { useEffect, useState } from "react";
import { Link, useFetcher } from "react-router";
import type { TaskPhaseGateOutcome } from "~/celeris/action-types";
import type {
  ExecutionView,
  ExecutionWorkUnitView,
  IntegrationCheckProgress,
  PhaseCheckpointView,
  PhaseGateAction,
  WorkUnitCheckLog,
} from "~/celeris/types";
import { FieldErrors, TaskPhaseGateFlash } from "~/components/Flash";
import { Badge, RoleLabel } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { hintClass, tableClass, tdClass, textareaClass, thClass, theadClass } from "~/components/ui/form";
import { Mono } from "~/components/ui/misc";
import {
  checkDoneLabel,
  checkProgressLine,
  checkpointSummary as checkpointSummaryLine,
  costReferenceLabel,
  currentWorkUnit,
  directExecutionSummary,
  EXECUTION_SECTION_LABEL,
  gateModeLabel,
  isRepairWorkUnit,
  PHASE_GATE_ACTION_LABEL,
  parallelSummaryLine,
  phaseCheckpointHeadline,
  phaseCheckpointNextLine,
  phaseCheckpointSections,
  planSummaryLine,
  planVersionLabel,
  quotaSummaryLines,
  shortCommit,
  WORK_UNIT_KIND_LABEL,
  WORK_UNIT_STATUS_TONE,
  workUnitGroups,
} from "~/lib/task-execution";
import { cn } from "~/lib/utils";

/**
 * タスク詳細の「実行」節（celeris ADR-0072 D19/D20）。計画の無い Task は「直接実行」の 1 行だけ、
 * 計画のある Task は WU の表・版の履歴（replan）を出す。`detail.execution` が `null`（events に
 * E-phase の活動が無い古いタスク）なら何も描かない（D23 の後方互換）。
 */
export function ExecutionSection({
  execution,
  taskId,
}: {
  execution: ExecutionView | null | undefined;
  /** 途中確認のボタンの送り先（`/tasks/:id` の action）。無ければボタンを出さない。 */
  taskId?: string;
}) {
  if (!execution) return null;
  const { plan, metrics } = execution;
  const gate = gateModeLabel(execution);
  // ADR-0074 D4（Phase F3 quota）: quota が主指標、定価 USD は参考（GUI (j)）。
  const quotaLines = quotaSummaryLines(metrics);
  const costLabel = costReferenceLabel(metrics);

  return (
    <section aria-labelledby="execution-heading" data-testid="execution-section">
      <Card>
        <CardHeader
          icon="zap"
          tone="primary"
          title={
            <h2 id="execution-heading" className="text-[0.95rem] font-semibold text-fg">
              {EXECUTION_SECTION_LABEL}
            </h2>
          }
          description={
            <span data-testid="execution-summary">
              {plan ? planSummaryLine(plan, metrics) : directExecutionSummary(metrics)}
            </span>
          }
        />
        <CardBody className="space-y-4">
          {execution.phase_checkpoint && (
            <PhaseCheckpointPanel checkpoint={execution.phase_checkpoint} taskId={taskId} />
          )}
          {gate && (
            <p className="text-sm text-fg-muted" data-testid="execution-gate">
              gate: {gate}
            </p>
          )}
          {(quotaLines.length > 0 || costLabel) && (
            <div data-testid="execution-quota" className="text-sm text-fg-muted">
              <p className="font-medium text-fg-subtle lg:text-xs">quota 消費</p>
              {quotaLines.length > 0 ? (
                <ul className="mt-1 space-y-0.5">
                  {quotaLines.map((line) => (
                    <li key={line} data-testid="execution-quota-line">
                      {line}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-1 text-fg-subtle" data-testid="execution-quota-none">
                  quota の記録はありません。
                </p>
              )}
              {costLabel && (
                <p className="mt-1 text-fg-subtle" data-testid="execution-cost-reference">
                  {costLabel}
                </p>
              )}
            </div>
          )}
          {!plan ? (
            <p className="text-sm text-fg-muted" data-testid="execution-no-plan">
              計画はありません（暗黙の WorkUnit で直接実行）。
            </p>
          ) : (
            <>
              {parallelSummaryLine(plan) && (
                <p className="text-sm text-fg-muted" data-testid="execution-parallel">
                  {parallelSummaryLine(plan)}
                </p>
              )}
              {/* ADR-0072 D20（Phase E5）: モバイル幅（393px）は表ではなくカードの一覧に折り返す
                  （`max-sm:`。`~/routes/projects.tsx` の案件一覧と同じ技法）。表のまま横スクロールさせると、
                  この中の `<details>`（checkpoint の折り畳み）へのキーボードフォーカスがブラウザの
                  ネイティブな「要素を可視領域へ」で横スクロールを動かし、タッチのスワイプ検査
                  （`mobile-audit` の `touch-scroll`）と競合する。カードにすれば横スクロール自体が無い。 */}
              <div className="overflow-x-auto sm:rounded-lg sm:border sm:border-border">
                <table className={cn(tableClass, "max-sm:block")} data-testid="work-unit-table">
                  <thead className={cn(theadClass, "max-sm:hidden")}>
                    <tr>
                      <th className={thClass}>key</th>
                      <th className={thClass}>工程</th>
                      <th className={thClass}>title</th>
                      <th className={thClass}>status</th>
                      <th className={thClass}>依存</th>
                      <th className={thClass}>担当</th>
                      <th className={thClass}>harness</th>
                      <th className={thClass}>model / lane</th>
                      <th className={thClass}>run</th>
                      <th className={thClass}>continuation</th>
                      <th className={thClass}>retry</th>
                      <th className={thClass}>checkpoint / 理由</th>
                    </tr>
                  </thead>
                  {/* ADR-0074 D1（Phase F2b）: v2 の計画は工程ごとに見出しを付けてまとめる。 */}
                  {workUnitGroups(plan).map((group) => (
                    <tbody key={group.key || "all"} className="max-sm:block" data-testid="work-unit-group">
                      {group.label && (
                        <tr className="max-sm:block" data-testid="work-unit-phase-heading">
                          <th
                            colSpan={12}
                            scope="colgroup"
                            className={cn(
                              thClass,
                              "bg-surface-2/60 text-left max-sm:block max-sm:border-t max-sm:border-border max-sm:px-3",
                            )}
                          >
                            {group.label}
                          </th>
                        </tr>
                      )}
                      {group.units.map((wu) => (
                        <WorkUnitRow
                          key={wu.id}
                          wu={wu}
                          isCurrent={currentWorkUnit(plan)?.id === wu.id}
                          taskId={taskId}
                        />
                      ))}
                    </tbody>
                  ))}
                </table>
              </div>

              {plan.versions.length > 1 && (
                <div data-testid="execution-plan-versions">
                  <p className="text-sm font-medium text-fg-subtle lg:text-xs">計画の版（replan の履歴）</p>
                  <ul className="mt-1 space-y-0.5 text-sm text-fg">
                    {plan.versions.map((v) => (
                      <li key={v.id} data-testid="execution-plan-version" className="break-words">
                        {planVersionLabel(v)}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}
        </CardBody>
      </Card>
    </section>
  );
}

/** モバイルのカード表示で、値の前に付ける列名（`~/routes/projects.tsx` の案件一覧と同じ技法）。 */
function CellLabel({ children }: { children: string }) {
  return <span className="text-fg-subtle sm:hidden">{children}: </span>;
}

const cardCellClass =
  "max-sm:block max-sm:border-0 max-sm:px-1 max-sm:first:col-span-2 max-sm:first:pl-1 max-sm:last:col-span-2 max-sm:last:pr-1 max-sm:break-words";

function WorkUnitRow({
  wu,
  isCurrent,
  taskId,
}: {
  wu: ExecutionWorkUnitView;
  isCurrent: boolean;
  /** 実行中の run のログ（`/tasks/:id/runs/:runId`）へのリンク先。 */
  taskId?: string;
}) {
  const repair = isRepairWorkUnit(wu);
  return (
    <tr
      data-testid="work-unit-row"
      className={cn(
        "max-sm:grid max-sm:grid-cols-2 max-sm:border-t max-sm:border-border max-sm:p-3",
        isCurrent && "bg-surface-2/60",
      )}
    >
      <td className={cn(tdClass, "font-mono text-xs", cardCellClass)}>
        <Mono>{wu.key}</Mono>
        {repair && (
          <Badge tone="warning" className="ml-1.5" data-testid="work-unit-repair-badge">
            repair
          </Badge>
        )}
        {wu.branch && (
          <span className="mt-0.5 block break-all text-fg-subtle" data-testid="work-unit-branch">
            <CellLabel>ブランチ</CellLabel>
            <Mono>{wu.branch}</Mono>
            {shortCommit(wu.head_commit ?? wu.integrated_commit) && (
              <span className="ml-1">@{shortCommit(wu.head_commit ?? wu.integrated_commit)}</span>
            )}
          </span>
        )}
      </td>
      <td className={cn(tdClass, cardCellClass)} data-testid="work-unit-phase">
        <CellLabel>工程</CellLabel>
        {wu.phase ?? "-"}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <span className="break-words">{wu.title}</span>
        <RoleLabel role={WORK_UNIT_KIND_LABEL[wu.kind] ?? wu.kind} className="ml-1.5" />
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <CellLabel>status</CellLabel>
        <Badge tone={WORK_UNIT_STATUS_TONE[wu.status]} dot data-testid="work-unit-status">
          {wu.status}
        </Badge>
        {wu.blocked_reason && <span className="ml-1.5 text-fg-subtle">（{wu.blocked_reason}）</span>}
        {wu.running_run_id && (
          <span className="mt-0.5 block break-all text-fg-subtle" data-testid="work-unit-running-run">
            run{" "}
            {taskId ? (
              <Link
                to={`/tasks/${taskId}/runs/${wu.running_run_id}`}
                className="inline-flex min-h-11 items-center break-all text-primary hover:underline lg:min-h-0"
                data-testid="work-unit-run-log-link"
              >
                <Mono className="text-primary">{wu.running_run_id}</Mono>
              </Link>
            ) : (
              <Mono>{wu.running_run_id}</Mono>
            )}
          </span>
        )}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <CellLabel>依存</CellLabel>
        {wu.depends_on.length > 0 ? wu.depends_on.join(", ") : "-"}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <CellLabel>担当</CellLabel>
        {wu.assignee ?? "-"}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <CellLabel>harness</CellLabel>
        {wu.harness ?? "-"}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        <CellLabel>model / lane</CellLabel>
        {[wu.model, wu.lane].filter(Boolean).join(" / ") || "-"}
      </td>
      <td className={cn(tdClass, "tabular-nums", cardCellClass)}>
        <CellLabel>run</CellLabel>
        {wu.runs}
      </td>
      <td className={cn(tdClass, "tabular-nums", cardCellClass)}>
        <CellLabel>continuation</CellLabel>
        {wu.continuations}
      </td>
      <td className={cn(tdClass, "tabular-nums", cardCellClass)}>
        <CellLabel>retry</CellLabel>
        {wu.retries}
      </td>
      <td className={cn(tdClass, cardCellClass)}>
        {/* ADR-0055 D1-4: 本文 14px 以上。モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
        <details data-testid="work-unit-checkpoint" className="text-sm text-fg-muted lg:text-xs">
          <summary className="cursor-pointer select-none break-words">
            {checkpointSummaryLine(wu.last_checkpoint)}
          </summary>
          {wu.last_checkpoint && (
            <pre className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap break-words rounded-md bg-surface-2 p-2 text-[0.7rem]">
              {JSON.stringify(wu.last_checkpoint, null, 2)}
            </pre>
          )}
        </details>
        {wu.check_progress && <WorkUnitCheckProgress progress={wu.check_progress} taskId={taskId} wuId={wu.id} />}
        {wu.last_reason && (
          <p className="mt-1 break-words text-sm text-fg-muted lg:text-xs" data-testid="work-unit-last-reason">
            {wu.last_reason}
          </p>
        )}
      </td>
    </tr>
  );
}

/** 実行中の検査の出力の末尾を読み直す間隔。 */
const CHECK_LOG_POLL_MS = 5000;

/**
 * 2026-10-04 統合の検査の進み具合 D4: 統合 WU の検査（現在の検査と済んだ検査）。統合は run を持たないので、
 * run のログの代わりに celeris の `check_progress` を出し、開いたときだけ出力の末尾
 * （`GET /tasks/{id}/work-units/{wu_id}/check-log`）を取りに行く。実行中は開いている間だけ数秒おきに読み直す。
 * 葉の WU の受け入れ検査（worker run の後に daemon が流す。2026-10-04 WU 検査の引き継ぎ D3）も同じ形で出す。
 */
function WorkUnitCheckProgress({
  progress,
  taskId,
  wuId,
}: {
  progress: IntegrationCheckProgress;
  taskId?: string;
  wuId: string;
}) {
  const [open, setOpen] = useState(false);
  const logFetcher = useFetcher<WorkUnitCheckLog>();
  const running = progress.current != null;
  const logUrl = taskId ? `/tasks/${taskId}/work-units/${encodeURIComponent(wuId)}/check-log` : null;
  const { load } = logFetcher;
  // 実行中は今の検査の番号を指して読む（検査が進めば URL が変わり、読み直す）。終わっていれば最後の検査。
  const currentIndex = progress.current?.index;
  const target = logUrl && currentIndex != null ? `${logUrl}?index=${currentIndex}` : logUrl;
  // 開いたら読む。実行中なら開いている間だけ間隔を置いて読み直す。
  useEffect(() => {
    if (!open || !target) return;
    load(target);
    if (!running) return;
    const timer = setInterval(() => load(target), CHECK_LOG_POLL_MS);
    return () => clearInterval(timer);
  }, [open, target, running, load]);
  const log = logFetcher.data;
  return (
    <div className="mt-1 space-y-1 text-sm lg:text-xs" data-testid="work-unit-check-progress">
      <p className={cn("break-words", running ? "text-fg" : "text-fg-muted")} data-testid="work-unit-check-line">
        {running && (
          <Badge tone="info" dot className="mr-1.5">
            検査中
          </Badge>
        )}
        {checkProgressLine(progress)}
      </p>
      {progress.finished.length > 0 && (
        <ul className="space-y-0.5 text-fg-muted" data-testid="work-unit-check-finished">
          {progress.finished.map((f) => (
            <li key={f.index} className="break-words">
              <span className={f.pass ? "text-success-soft-fg" : "text-danger-soft-fg"}>{checkDoneLabel(f)}</span>{" "}
              <Mono>{f.cmd}</Mono>
            </li>
          ))}
        </ul>
      )}
      {logUrl && (
        <details
          data-testid="work-unit-check-log"
          onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
        >
          <summary className="inline-flex min-h-11 cursor-pointer select-none items-center text-primary lg:min-h-0">
            {running ? "実行中の検査の出力の末尾" : "最後の検査の出力の末尾"}
          </summary>
          {log ? (
            <div className="mt-1">
              <p className={hintClass}>
                {`検査 ${log.index + 1}/${log.total}`} <Mono>{log.cmd}</Mono>
                {log.running ? "（実行中）" : log.exit != null ? `（exit ${log.exit}）` : ""}
                {log.truncated ? `・末尾のみ（全 ${log.size} バイト）` : ""}
              </p>
              <pre
                className="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-surface-2 p-2 font-mono text-[0.7rem]"
                data-testid="work-unit-check-log-tail"
              >
                {log.tail || "（まだ出力はありません）"}
              </pre>
            </div>
          ) : (
            <p className={hintClass}>{logFetcher.state === "idle" ? "" : "読み込み中…"}</p>
          )}
        </details>
      )}
    </div>
  );
}

const PHASE_GATE_BUTTONS: { action: PhaseGateAction; variant: "primary" | "secondary" | "danger" }[] = [
  { action: "continue", variant: "primary" },
  { action: "replan", variant: "secondary" },
  { action: "withdraw", variant: "danger" },
];

/**
 * celeris ADR-0074 D2.3/D2.4（Phase F3 途中確認）: 工程の後で止まった Task の途中報告と 3 つのボタン
 * （続ける / replan / 取り下げる）。報告は celeris が決定的に組み立てたものをそのまま並べる。
 * **GUI は検証しない**（replan の指示が空なら celeris が 422 を返し、その文言を欄の下に出す）。
 */
function PhaseCheckpointPanel({ checkpoint, taskId }: { checkpoint: PhaseCheckpointView; taskId?: string }) {
  const fetcher = useFetcher<TaskPhaseGateOutcome>({ key: `task-phase-gate-${taskId ?? "none"}` });
  const submitting = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  return (
    <div
      data-testid="phase-checkpoint"
      className="space-y-3 rounded-lg border border-warning-border bg-warning-soft p-3 text-sm"
    >
      <p className="font-semibold text-warning-soft-fg" data-testid="phase-checkpoint-headline">
        {phaseCheckpointHeadline(checkpoint)}
      </p>
      <p className="text-fg" data-testid="phase-checkpoint-next">
        {phaseCheckpointNextLine(checkpoint)}
      </p>
      {phaseCheckpointSections(checkpoint).map((section) => (
        <div key={section.title} data-testid="phase-checkpoint-section">
          <p className="font-medium text-fg-subtle lg:text-xs">{section.title}</p>
          <ul className="mt-1 space-y-0.5 break-words text-fg-muted">
            {section.lines.map((line, i) => (
              // 行は celeris が決定的に並べたもの。同じ文言が並ぶこともあるので添字を key に含める。
              // biome-ignore lint/suspicious/noArrayIndexKey: 並び替えない静的な一覧
              <li key={`${i}-${line}`}>{line}</li>
            ))}
          </ul>
        </div>
      ))}
      {taskId && checkpoint.report_idx != null && (
        <p>
          <a
            href={`/files/tasks/${taskId}/artifacts/${checkpoint.report_idx}`}
            // 警告色の背景の上では primary の文字色が暗色テーマで 4.5:1 に届かない（mobile-audit）ので、
            // 本文色＋下線でリンクと分かるようにする。
            className="inline-flex min-h-11 items-center font-medium text-fg underline"
            data-testid="phase-checkpoint-report-link"
          >
            途中報告（Markdown）を開く
          </a>
        </p>
      )}
      {taskId && (
        <fetcher.Form method="post" action={`/tasks/${taskId}`} className="space-y-2" data-testid="phase-gate-form">
          <input type="hidden" name="intent" value="phase_gate" />
          <label className="block">
            <span className="font-medium text-fg">人の指示（replan では必須、続けるときは任意）</span>
            <textarea name="note" rows={2} className={textareaClass} data-testid="phase-gate-note" />
          </label>
          <FieldErrors error={error} field="note" />
          <div className="flex flex-wrap gap-2">
            {PHASE_GATE_BUTTONS.map(({ action, variant }) => (
              <Button
                key={action}
                type="submit"
                name="phase_action"
                value={action}
                variant={variant}
                size="sm"
                disabled={submitting}
                data-testid={`phase-gate-${action}`}
              >
                {PHASE_GATE_ACTION_LABEL[action]}
              </Button>
            ))}
          </div>
          <p className={hintClass}>
            取り下げると worktree
            とブランチを片付けます。部分成果を残したいときは、取り下げる前に「変更」タブから取り込み（merge /
            PR）してください。
          </p>
        </fetcher.Form>
      )}
      <TaskPhaseGateFlash outcome={fetcher.data} />
    </div>
  );
}
