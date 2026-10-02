import { lazy, Suspense } from "react";
import { Link, type useFetcher } from "react-router";
import type { RetryOutcome, TransitionOutcome } from "~/celeris/action-types";
import type { ApprovalItem, BrowserRun, MilestoneView, OrgNode, TaskDetail, TaskRef } from "~/celeris/types";
import type { LiveViewState } from "~/components/BrowserRunsPanel";
/* celeris ADR-0072 D19/D20（Phase E5）: 実行の分解（Execution 節・ExecutionPhase）。 */
import { ExecutionSection } from "~/components/ExecutionSection";
import { RetryFlash, TransitionFlash } from "~/components/Flash";
import { LocalTime } from "~/components/LocalTime";
import { Badge, KindBadge, RoleLabel } from "~/components/ui/badge";
import { Button, buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import {
  checkboxClass,
  tableClass,
  tdClass,
  textareaClass,
  thClass,
  theadClass,
  touchLinkClass,
  trHoverClass,
} from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { DataItem, DataList, EmptyState, Mono } from "~/components/ui/misc";
import { Skeleton } from "~/components/ui/skeleton";
import type { Tone } from "~/components/ui/tone";
/* celeris ADR-0072 D19/D20（Phase E5）: 実行の分解（Execution 節・ExecutionPhase）。 */
import { isGateCandidate } from "~/lib/execution-mode";
import { shortId } from "~/lib/format";
import { runEndLabel, runEndTone } from "~/lib/task-execution";
import { cn } from "~/lib/utils";
import { ACTION_LABELS } from "./labels";
import { TaskEditSection } from "./TaskEditSection";
import { WriteSetSection } from "./WriteSetSection";

const BrowserRunsPanel = lazy(() =>
  import("~/components/BrowserRunsPanel").then((m) => ({ default: m.BrowserRunsPanel })),
);

// Phase F6-fix（ADR-0055 性能予算）: 「実行の形」カード（`~/components/ExecutionModeControl`・
// `~/lib/execution-mode.ts`・確認文言）は gate の対象になるタスクでしか出ない補助の操作なので、初回の JS に載せず
// 別チャンクにする（F6 で task 系ルートの初回 JS が 532KB の予算を 5KB 超えた）。
const ExecutionModeControl = lazy(() =>
  import("~/components/ExecutionModeControl").then((m) => ({ default: m.ExecutionModeControl })),
);

// 同じく F6-fix: 人の判断待ちの確認パネル（`~/components/HumanReviewPanel`）は review タスクで判断待ちが
// あるときだけ出るので、それ以外のタスクの初回 JS から外す（予算に余裕を残すため）。
const HumanReviewPanel = lazy(() =>
  import("~/components/HumanReviewPanel").then((m) => ({ default: m.HumanReviewPanel })),
);

/**
 * ADR-0055 D1（393px で崩れない）: run 一覧を `max-sm:` でカードにするときの各 `<td>` の共通クラス
 * （`~/routes/projects.tsx` の案件一覧と同じ技法。celeris ADR-0072 D20/Phase E5）。
 */
const runCardCellClass = "max-sm:block max-sm:border-0 max-sm:px-1 max-sm:first:col-span-2 max-sm:break-words";

/** モバイルのカード表示で、値の前に付ける列名。 */
function RunCellLabel({ children }: { children: string }) {
  return <span className="text-fg-subtle sm:hidden">{children}: </span>;
}

/** run の outcome → 色（docs/adr/0011 D4 と同じ考え方。文字列は outcome 名をそのまま出す）。 */
const OUTCOME_TONE: Record<string, Tone> = {
  done: "success",
  question: "info",
  error: "danger",
  requeue: "warning",
  lease_expired: "warning",
  // ADR-0044 D2/D8: 人のコメントで止めた run は**失敗ではない**ので danger にしない。
  interrupted: "info",
};

/** 概要タブ（ADR-0044 D5）: 従来の詳細一式に、人が直接直せる編集フォーム（D1）を足したもの。 */
export function OverviewTab({
  browserRuns,
  liveViews,
  browserOwner,
  detail,
  artifactCount,
  org,
  milestones,
  genres,
  fetcher,
  submitting,
  retryFetcher,
  retrying,
  fetchedAt,
  humanReview,
}: {
  humanReview: ApprovalItem[];
  detail: TaskDetail;
  browserRuns: BrowserRun[];
  liveViews: Record<string, LiveViewState>;
  browserOwner: { csrfToken: string | null };
  artifactCount: number;
  org: OrgNode[];
  milestones: MilestoneView[];
  genres: string[];
  fetcher: ReturnType<typeof useFetcher<TransitionOutcome>>;
  submitting: boolean;
  retryFetcher: ReturnType<typeof useFetcher<RetryOutcome>>;
  retrying: boolean;
  fetchedAt: string;
}) {
  const { task } = detail;
  return (
    <>
      {/* Phase F6-fix: `React.lazy`（このファイル冒頭）なので `Suspense` で包む。 */}
      {humanReview.length > 0 && (
        <Suspense fallback={<Skeleton className="h-32 w-full" />}>
          <HumanReviewPanel
            items={humanReview}
            criteria={detail.criteria}
            priorReview={detail.prior_review}
            reviewTaskId={task.id}
          />
        </Suspense>
      )}

      <section aria-labelledby="info-heading" data-testid="info-section">
        <Card>
          <CardHeader
            icon="file"
            tone="neutral"
            title={
              <h2 id="info-heading" className="text-[0.95rem] font-semibold text-fg">
                基本情報
              </h2>
            }
          />
          <CardBody className="space-y-4">
            <DataItem label="目的" wide>
              <p className="whitespace-pre-wrap text-sm" data-testid="task-objective">
                {task.objective}
              </p>
            </DataItem>
            {/* フェーズ 71（ADR-0055 D2 ラウンド 3）: メタデータは 2 列を既定にする（`~/components/ui/misc.tsx`
                の既定 `DataList` は `sm:`（640px）未満は 1 列なので、393px の実機では常に 1 列になってしまう）。
                374px 未満のごく狭い端末だけ 1 列に折り返す。 */}
            <DataList className="grid-cols-1 min-[374px]:grid-cols-2 sm:grid-cols-2 lg:grid-cols-4">
              <DataItem label="priority">{`${detail.priority_label}（${task.priority}）`}</DataItem>
              <DataItem label="worker_hint">
                {`tier=${task.worker_hint.tier}${task.worker_hint.adapter ? `, adapter=${task.worker_hint.adapter}` : ""}`}
              </DataItem>
              <DataItem label="attempts / max_retries">
                <span className="tabular-nums">{`${task.attempts} / ${task.budget.max_retries}`}</span>
              </DataItem>
              <DataItem label="budget">
                {`max_turns=${task.budget.max_turns}, max_wall_secs=${task.budget.max_wall_secs}, max_retries=${task.budget.max_retries}`}
              </DataItem>
              <DataItem label="成果物">
                <span className="tabular-nums">{artifactCount}</span>
              </DataItem>
              <DataItem label="workspace_dir" wide>
                <span className="break-all font-mono text-xs">{detail.workspace_dir ?? "(remote)"}</span>
              </DataItem>
            </DataList>
          </CardBody>
        </Card>
      </section>

      {/* ADR-0044 D1（Phase 53）: 人がタスクを細かく直せる。終端のタスクは celeris が 409 を返すので出さない。 */}
      {detail.actions.includes("edit") && (
        <TaskEditSection key={task.updated_at} detail={detail} org={org} milestones={milestones} genres={genres} />
      )}

      {/* docs/adr/0130 D1/D2/D4: expected/actual write-set と target からの behind commits・age。 */}
      <WriteSetSection detail={detail} />

      <section data-testid="relations-section">
        <Card>
          <CardHeader
            icon="gitBranch"
            tone="teal"
            title={<h2 className="text-[0.95rem] font-semibold text-fg">子 / 依存</h2>}
          />
          <CardBody className="space-y-5">
            <TaskRefList label="dependencies" testId="dependencies" refs={detail.dependencies} />
            <TaskRefList label="dependents" testId="dependents" refs={detail.dependents} />
            <TaskRefList label="children" testId="children" refs={detail.children} />
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="timers-heading" data-testid="timers-section">
        <Card>
          <CardHeader
            icon="clock"
            tone="info"
            title={
              <h2 id="timers-heading" className="text-[0.95rem] font-semibold text-fg">
                タイマー
              </h2>
            }
          />
          <CardBody>
            <DataList className="grid-cols-1 min-[374px]:grid-cols-2 sm:grid-cols-2 lg:grid-cols-4">
              <DataItem label="lease_expires_at">{detail.timers.lease_expires_at ?? "-"}</DataItem>
              <DataItem label="backoff_until">{detail.timers.backoff_until ?? "-"}</DataItem>
              <DataItem label="consecutive_requeues / max_requeues">
                <span className="tabular-nums">
                  {`${detail.timers.consecutive_requeues} / ${detail.timers.max_requeues}`}
                </span>
              </DataItem>
              <DataItem label="consecutive_reviewer_requeues">
                <span className="tabular-nums">{String(detail.timers.consecutive_reviewer_requeues)}</span>
              </DataItem>
              <DataItem label="now">{detail.timers.now}</DataItem>
            </DataList>
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="criteria-heading" data-testid="criteria-section">
        <Card>
          <CardHeader
            icon="checkCircle"
            tone="success"
            title={
              <h2 id="criteria-heading" className="text-[0.95rem] font-semibold text-fg">
                受け入れ条件と判定
              </h2>
            }
          />
          <CardBody>
            {detail.criteria.length === 0 ? (
              <EmptyState icon="checkCircle" title="ありません。" compact />
            ) : (
              <ul className="space-y-3">
                {detail.criteria.map((criterion) => (
                  <li
                    key={criterion.idx}
                    data-testid="criterion-item"
                    className="rounded-lg border border-border bg-surface-2/40 p-3 text-sm"
                  >
                    <p className="flex flex-wrap items-center gap-2">
                      <Mono>#{criterion.idx}</Mono>
                      <KindBadge kind={criterion.check.type} />
                      <span className="text-fg">{criterion.text}</span>
                    </p>
                    {criterion.latest_verdict && (
                      <p
                        className="mt-1.5 flex flex-wrap items-center gap-1.5 text-fg-muted"
                        data-testid="criterion-verdict"
                      >
                        <span>直近判定:</span>
                        <Badge tone={criterion.latest_verdict.pass ? "success" : "danger"} dot>
                          {criterion.latest_verdict.pass ? "pass" : "fail"}
                        </Badge>
                        <span>— {criterion.latest_verdict.reason}</span>
                      </p>
                    )}
                    {criterion.check.type === "human" && criterion.approval && (
                      <p className="mt-1.5 text-fg-muted" data-testid="criterion-approval">
                        Approval:{" "}
                        <Link
                          to={`/tasks/${criterion.approval.approval.id}`}
                          className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                        >
                          {criterion.approval.approval.id}
                        </Link>
                        （{criterion.approval.approval.status}）
                      </p>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </CardBody>
        </Card>
      </section>

      {/* celeris ADR-0072 D19/D20（Phase E5）: 実行の分解（計画・WU の表・replan の履歴）。
          計画も gate の判定も無い古いタスクは execution が無いので何も出ない（D23 の後方互換）。 */}
      <ExecutionSection execution={detail.execution} taskId={task.id} />
      {(isGateCandidate(task) || detail.execution?.plan_approval) && (
        <Suspense fallback={null}>
          <ExecutionModeControl task={task} execution={detail.execution} actions={detail.actions} />
        </Suspense>
      )}
      {browserRuns.length > 0 && (
        <Suspense fallback={null}>
          <BrowserRunsPanel runs={browserRuns} liveViews={liveViews} csrfToken={browserOwner.csrfToken} />
        </Suspense>
      )}

      <section aria-labelledby="runs-heading" data-testid="runs-section">
        <Card>
          <CardHeader
            icon="terminal"
            title={
              <h2 id="runs-heading" className="text-[0.95rem] font-semibold text-fg">
                run 一覧
              </h2>
            }
          />
          <CardBody className={detail.runs.length === 0 ? undefined : "p-0"}>
            {detail.runs.length === 0 ? (
              <EmptyState icon="terminal" title="ありません。" compact />
            ) : (
              /* ADR-0055 D1（393px で崩れない）: モバイルは表ではなくカードの一覧に折り返す（`max-sm:`。
                 `~/routes/projects.tsx` の案件一覧と同じ技法。celeris ADR-0072 D20/Phase E5 で列が増え、
                 表のまま横スクロールさせると `<details>`（run-outcome-detail）へのキーボードフォーカスが
                 ブラウザのネイティブな「要素を可視領域へ」で横スクロールを動かし、タッチのスワイプ検査
                 〈mobile-audit の touch-scroll〉と競合するため、この列数では表を維持しない）。 */
              <div className="overflow-x-auto sm:rounded-b-xl">
                <table className={cn(tableClass, "max-sm:block")}>
                  <thead className={cn(theadClass, "max-sm:hidden")}>
                    <tr>
                      <th className={thClass}>run_id</th>
                      <th className={thClass}>role</th>
                      <th className={thClass}>adapter</th>
                      <th className={thClass}>provider</th>
                      <th className={thClass}>account</th>
                      <th className={thClass}>model</th>
                      <th className={thClass}>started_at</th>
                      <th className={thClass}>finished_at</th>
                      <th className={thClass}>outcome</th>
                      {/* celeris ADR-0072 D19/D20（Phase E5）: 構造化した終わり方と WU の key。 */}
                      <th className={thClass}>end</th>
                      <th className={thClass}>WU</th>
                      <th className={thClass}>usage</th>
                      <th className={thClass}>progress</th>
                      <th className={thClass}>artifacts</th>
                      <th className={thClass}>verdicts</th>
                      <th className={thClass}>files</th>
                      <th className={thClass}>ログ</th>
                    </tr>
                  </thead>
                  <tbody className="max-sm:block">
                    {detail.runs.map((run) => (
                      <tr
                        key={run.run_id}
                        data-testid="run-row"
                        className={cn(
                          trHoverClass,
                          "max-sm:grid max-sm:grid-cols-2 max-sm:border-t max-sm:border-border max-sm:p-3",
                        )}
                      >
                        <td className={cn(tdClass, "font-mono text-xs break-all", runCardCellClass)} title={run.run_id}>
                          {shortId(run.run_id)}
                        </td>
                        <td className={cn(tdClass, runCardCellClass)}>
                          <RunCellLabel>role</RunCellLabel>
                          <RoleLabel role={run.role} />
                        </td>
                        <td className={cn(tdClass, runCardCellClass)}>
                          <RunCellLabel>adapter</RunCellLabel>
                          {run.adapter}
                        </td>
                        <td className={cn(tdClass, runCardCellClass)}>
                          <RunCellLabel>provider</RunCellLabel>
                          {run.provider ?? "-"}
                        </td>
                        {/* プールの run だけ、どのアカウントで動いたかが入る（ADR-0024 D4 / ADR-0025） */}
                        <td className={cn(tdClass, "whitespace-nowrap", runCardCellClass)} data-testid="run-account">
                          <RunCellLabel>account</RunCellLabel>
                          {run.account ?? "-"}
                        </td>
                        <td className={cn(tdClass, runCardCellClass)}>
                          <RunCellLabel>model</RunCellLabel>
                          {run.model}
                        </td>
                        {/* フェーズ 74（ADR-0055 D2 ラウンド 6）: 生の ISO は表の幅も取るので相対表示に揃える。
                            ADR-0055 D1-4: 本文 14px 以上。モバイルは text-sm、デスクトップは lg: で元の text-xs のまま
                            （celeris ADR-0072 D20/Phase E5 で runs 一覧が実データを持つ経路が増え、この列が初めて
                            機械検査対象になって見つかった既存の欠落。ここで合わせて直す）。 */}
                        <td
                          className={cn(
                            tdClass,
                            "whitespace-nowrap text-sm text-fg-subtle lg:text-xs",
                            runCardCellClass,
                          )}
                        >
                          <RunCellLabel>started_at</RunCellLabel>
                          <LocalTime iso={run.started_at} fetchedAtIso={fetchedAt} />
                        </td>
                        <td
                          className={cn(
                            tdClass,
                            "whitespace-nowrap text-sm text-fg-subtle lg:text-xs",
                            runCardCellClass,
                          )}
                        >
                          <RunCellLabel>finished_at</RunCellLabel>
                          {run.finished_at ? <LocalTime iso={run.finished_at} fetchedAtIso={fetchedAt} /> : "-"}
                        </td>
                        <td className={cn(tdClass, runCardCellClass, "max-sm:col-span-2")}>
                          {run.outcome ? (
                            <Badge tone={OUTCOME_TONE[run.outcome] ?? "neutral"} title={run.outcome_text ?? undefined}>
                              {run.outcome}
                            </Badge>
                          ) : (
                            <span className="text-fg-subtle">-</span>
                          )}
                          {/* 長い理由・要約はステータス欄に混ぜず、折り畳みの中に分ける。 */}
                          {run.outcome_text && (
                            <details
                              data-testid="run-outcome-detail"
                              className="mt-1 max-w-xs text-sm text-fg-muted lg:text-xs"
                            >
                              <summary className="cursor-pointer select-none">詳細</summary>
                              <p className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap break-words">
                                {run.outcome_text}
                              </p>
                            </details>
                          )}
                        </td>
                        {/* celeris ADR-0072 D19/D20（Phase E5）: `RunEnd`（無ければ導入前・分類できなかった run）。 */}
                        <td className={cn(tdClass, runCardCellClass)} data-testid="run-end">
                          <RunCellLabel>end</RunCellLabel>
                          {runEndLabel(run.end) ? (
                            <Badge tone={runEndTone(run.end)}>{runEndLabel(run.end)}</Badge>
                          ) : (
                            <span className="text-fg-subtle">-</span>
                          )}
                        </td>
                        <td className={cn(tdClass, "font-mono text-xs", runCardCellClass)} data-testid="run-work-unit">
                          <RunCellLabel>WU</RunCellLabel>
                          {run.work_unit ?? "-"}
                        </td>
                        <td className={cn(tdClass, "tabular-nums", runCardCellClass)}>
                          <RunCellLabel>usage</RunCellLabel>
                          {run.usage
                            ? `in=${run.usage.input_tokens ?? "-"} out=${run.usage.output_tokens ?? "-"}`
                            : "-"}
                        </td>
                        <td className={cn(tdClass, "tabular-nums", runCardCellClass)}>
                          <RunCellLabel>progress</RunCellLabel>
                          {run.progress}
                        </td>
                        <td className={cn(tdClass, "tabular-nums", runCardCellClass)}>
                          <RunCellLabel>artifacts</RunCellLabel>
                          {run.artifacts}
                        </td>
                        <td className={cn(tdClass, "tabular-nums", runCardCellClass)}>
                          <RunCellLabel>verdicts</RunCellLabel>
                          {run.verdicts}
                        </td>
                        <td className={cn(tdClass, runCardCellClass)} data-testid="run-files">
                          <RunCellLabel>files</RunCellLabel>
                          {run.files
                            ? ["stdout", "stderr", "result"]
                                .filter((k) => run.files?.[k as keyof typeof run.files])
                                .join(", ") || "-"
                            : "-"}
                        </td>
                        <td className={cn(tdClass, runCardCellClass, "max-sm:col-span-2")}>
                          <Link
                            to={`/tasks/${task.id}/runs/${run.run_id}`}
                            data-testid="run-log-link"
                            className={buttonClass({ variant: "ghost", size: "xs" })}
                          >
                            <Icon name="terminal" />
                            ログ
                          </Link>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="delegated-heading" data-testid="delegated-section">
        <Card>
          <CardHeader
            icon="users"
            tone="teal"
            title={
              <h2 id="delegated-heading" className="text-[0.95rem] font-semibold text-fg">
                委譲
              </h2>
            }
          />
          <CardBody>
            {detail.delegated.length === 0 ? (
              <EmptyState icon="users" title="ありません。" compact />
            ) : (
              <ul className="space-y-3">
                {detail.delegated.map((group) => (
                  <li
                    key={group.run_id}
                    data-testid="delegated-group"
                    data-run-id={group.run_id}
                    className="rounded-lg border border-border p-3 text-sm"
                  >
                    {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
                    <p className="text-sm text-fg-subtle lg:text-xs">
                      run{" "}
                      <Link
                        to={`/tasks/${task.id}/runs/${group.run_id}`}
                        className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                      >
                        {group.run_id}
                      </Link>{" "}
                      · {group.ts}
                    </p>
                    <ul className="mt-2 space-y-1.5">
                      {group.tasks.map((child) => (
                        <li key={child.id} className="flex flex-wrap items-center gap-1.5">
                          <Link
                            to={`/tasks/${child.id}`}
                            data-testid="delegated-child-link"
                            className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                          >
                            {child.title}
                          </Link>
                          <span className="text-fg-subtle">（{child.status}）</span>
                        </li>
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
            )}
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="prior-review-heading" data-testid="prior-review-section">
        <Card>
          <CardHeader
            icon="rotate"
            title={
              <h2 id="prior-review-heading" className="text-[0.95rem] font-semibold text-fg">
                prior_review
              </h2>
            }
          />
          <CardBody>
            {detail.prior_review.length === 0 ? (
              <EmptyState icon="rotate" title="ありません。" compact />
            ) : (
              <ul className="space-y-1.5">
                {detail.prior_review.map((note) => (
                  <li
                    key={`${note.criterion}-${note.pass}-${note.reason}`}
                    data-testid="prior-review-item"
                    className="flex flex-wrap items-center gap-1.5 text-sm"
                  >
                    <Mono>#{note.criterion}</Mono>
                    <Badge tone={note.pass ? "success" : "danger"} dot>
                      {note.pass ? "pass" : "fail"}
                    </Badge>
                    <span className="text-fg-muted">— {note.reason}</span>
                  </li>
                ))}
              </ul>
            )}
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="answers-heading" data-testid="answers-section">
        <Card>
          <CardHeader
            icon="message"
            tone="info"
            title={
              <h2 id="answers-heading" className="text-[0.95rem] font-semibold text-fg">
                answers
              </h2>
            }
          />
          <CardBody className="space-y-3">
            {detail.answers.length === 0 ? (
              <EmptyState icon="message" title="ありません。" compact />
            ) : (
              <ul className="space-y-1.5">
                {detail.answers.map((note) => (
                  <li
                    key={`${note.question}-${note.answer}`}
                    data-testid="answer-item"
                    className="rounded-lg border border-border p-2.5 text-sm text-fg"
                  >
                    Q: {note.question} / A: {note.answer}
                  </li>
                ))}
              </ul>
            )}
            {detail.latest_question && (
              <p className="text-sm text-fg-muted" data-testid="latest-question">
                最新の質問: {detail.latest_question}
              </p>
            )}
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="actions-heading" data-testid="actions-section">
        <Card>
          <CardHeader
            icon="zap"
            tone="warning"
            title={
              <h2 id="actions-heading" className="text-[0.95rem] font-semibold text-fg">
                操作
              </h2>
            }
          />
          <CardBody className="space-y-4">
            <TransitionFlash outcome={fetcher.data} />
            {/* フェーズ 71（ADR-0055 D2 ラウンド 3）: 操作は画面下の全幅ボタン（モバイルは縦積み、
                `sm:` からは元どおり横並び）。 */}
            {detail.actions.length === 0 ? (
              <EmptyState icon="ban" title="できる操作はありません。" compact />
            ) : (
              <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:gap-4">
                {detail.actions.includes("approve") && (
                  <fetcher.Form
                    method="post"
                    className="flex w-full flex-col gap-2 rounded-lg border border-border p-3 sm:w-auto sm:max-w-xs"
                  >
                    <input type="hidden" name="intent" value="approve" />
                    <input type="hidden" name="expected_status" value={task.status} />
                    <textarea
                      name="note"
                      data-testid="action-note-approve"
                      rows={2}
                      placeholder="メモ（任意）"
                      className={textareaClass}
                    />
                    <Button
                      type="submit"
                      variant="success"
                      size="sm"
                      disabled={submitting}
                      data-testid="action-approve"
                      className="w-full sm:w-auto"
                    >
                      <Icon name="check" />
                      {ACTION_LABELS.approve}
                    </Button>
                  </fetcher.Form>
                )}
                {detail.actions.includes("reject") && (
                  <fetcher.Form
                    method="post"
                    className="flex w-full flex-col gap-2 rounded-lg border border-border p-3 sm:w-auto sm:max-w-xs"
                  >
                    <input type="hidden" name="intent" value="reject" />
                    <input type="hidden" name="expected_status" value={task.status} />
                    <textarea
                      name="note"
                      data-testid="action-note-reject"
                      rows={2}
                      placeholder="メモ（任意）"
                      className={textareaClass}
                    />
                    <Button
                      type="submit"
                      variant="danger"
                      size="sm"
                      disabled={submitting}
                      data-testid="action-reject"
                      className="w-full sm:w-auto"
                    >
                      <Icon name="x" />
                      {ACTION_LABELS.reject}
                    </Button>
                  </fetcher.Form>
                )}
                {detail.actions.includes("answer") && (
                  <fetcher.Form
                    method="post"
                    className="flex w-full flex-col gap-2 rounded-lg border border-border p-3 sm:w-auto sm:max-w-sm"
                  >
                    {detail.latest_question && (
                      <p className="text-sm text-fg" data-testid="action-question">
                        {detail.latest_question}
                      </p>
                    )}
                    <input type="hidden" name="intent" value="answer" />
                    <input type="hidden" name="expected_status" value={task.status} />
                    <textarea name="answer" data-testid="action-answer" rows={3} className={textareaClass} />
                    <Button
                      type="submit"
                      variant="primary"
                      size="sm"
                      disabled={submitting}
                      data-testid="action-answer-submit"
                      className="w-full sm:w-auto"
                    >
                      <Icon name="send" />
                      回答する
                    </Button>
                  </fetcher.Form>
                )}
                {detail.actions.includes("cancel") && (
                  <fetcher.Form
                    method="post"
                    className="flex w-full flex-col gap-2 rounded-lg border border-border p-3 sm:w-auto sm:max-w-xs"
                  >
                    <input type="hidden" name="intent" value="cancel" />
                    <input type="hidden" name="expected_status" value={task.status} />
                    <Button
                      type="submit"
                      variant="danger"
                      size="sm"
                      disabled={submitting}
                      data-testid="action-cancel"
                      className="w-full sm:w-auto"
                    >
                      <Icon name="ban" />
                      {ACTION_LABELS.cancel}
                    </Button>
                  </fetcher.Form>
                )}
                {detail.actions.includes("retry") && (
                  <retryFetcher.Form
                    method="post"
                    className="flex w-full flex-col gap-2 rounded-lg border border-border p-3 sm:w-auto sm:max-w-xs"
                  >
                    <input type="hidden" name="intent" value="retry" />
                    {/* ADR-0070 D2 追記（Phase 116）: 既定は ready。draft のまま始めたいときだけ
                        チェックする（既定を逆にした。チェック無し = ready）。
                        `min-h-11`: タップ領域 44 以上（mobile-audit で検出。他の同種チェックボックス
                        〈inbox.tsx / projects.$id.tsx〉と同じ）。 */}
                    <label className="flex min-h-11 items-center gap-2 text-sm text-fg">
                      <input type="checkbox" name="draft" value="true" className={checkboxClass} />
                      下書き（draft）のまま始める（既定は受け入れ済み = ready）
                    </label>
                    <Button
                      type="submit"
                      variant="primary"
                      size="sm"
                      disabled={retrying}
                      data-testid="action-retry"
                      className="w-full sm:w-auto"
                    >
                      <Icon name="rotate" />
                      {ACTION_LABELS.retry}
                    </Button>
                  </retryFetcher.Form>
                )}
              </div>
            )}
            <RetryFlash outcome={retryFetcher.data} />
          </CardBody>
        </Card>
      </section>

      {detail.worker_run_hint && (
        <section aria-labelledby="worker-run-hint-heading" data-testid="worker-run-hint-section">
          <Card>
            <CardHeader
              icon="cpu"
              title={
                <h2 id="worker-run-hint-heading" className="text-[0.95rem] font-semibold text-fg">
                  worker_run_hint
                </h2>
              }
            />
            <CardBody>
              <code
                className="block break-all rounded-lg bg-surface-2 p-3 font-mono text-xs text-fg"
                data-testid="worker-run-hint"
              >
                {detail.worker_run_hint}
              </code>
            </CardBody>
          </Card>
        </section>
      )}
    </>
  );
}

function TaskRefList({ label, testId, refs }: { label: string; testId: string; refs: TaskRef[] }) {
  return (
    <div data-testid={testId}>
      {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
      <p className="text-sm font-semibold uppercase tracking-wide text-fg-subtle lg:text-xs">{label}</p>
      {refs.length === 0 ? (
        <p className="mt-1 text-sm text-fg-subtle">ありません。</p>
      ) : (
        <ul className="mt-1.5 divide-y divide-border overflow-hidden rounded-lg border border-border">
          {refs.map((ref) => (
            <li key={ref.id} className="px-3 py-2 text-sm">
              <Link to={`/tasks/${ref.id}`} className={cn(touchLinkClass, "font-medium text-primary hover:underline")}>
                {ref.title}
              </Link>
              <span className="text-fg-subtle">（{ref.status}）</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
