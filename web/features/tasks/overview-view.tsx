import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import type { Action, RunSummary, TaskDetail, TaskExecutionView, WorkUnitView } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { buttonVariants } from "../../components/ui/button";
import { DataList, type DataListItem } from "../../components/ui/data-list";
import { Drawer } from "../../components/ui/drawer";
import { StatusBadge } from "../../components/ui/status-badge";
import { executionQuery } from "./execution-panel";
import { integrationRepairDisplay, integrationRepairTone } from "./integration-repair";
import { IntegrationRepairPanel } from "./integration-repair-panel";
import { taskDetailQuery } from "./task-detail-query";
import { type MobileSection, mobileSectionClass } from "./task-detail-tabs";

// /tasks/:id の概要（P3-08）と観測性の header・木（2026-10-04 screens-ops）。
// 長時間の task で「いまどうなっているか・次に何をするか」を header で一目に、親子 task・段・WU・
// main から取り込んだ integration repair を 1 本の木で示す。長い ID は等幅で省略し title 属性で全文を残す。
// 枠の中で折り返し・省略して、ページ全体の横溢れを出さない（S3）。
// スマホ幅では区画（概要・木）を切り替え、WU の詳細と integration repair は Drawer で開く（desktop は inline）。

const card = "min-w-0 rounded-lg border border-border bg-surface p-4";
const heading = "text-section font-semibold text-foreground";
const textLink = "break-words text-primary underline underline-offset-2";

/** 長い ID を等幅で 1 行に省略する。全文は title 属性（hover・長押し）と読み上げで読める。 */
export function ShortId({ id, className = "" }: { id: string; className?: string }) {
  return (
    <code
      title={id}
      data-slot="short-id"
      className={`inline-block max-w-full truncate align-bottom font-mono text-label ${className}`}
    >
      {id}
    </code>
  );
}

/** 現在の run: 走っている run（終わっていない）を優先し、無ければ最後に始まった run。 */
export function currentRun(runs: readonly RunSummary[]): RunSummary | null {
  const running = runs.filter((run) => !run.finished_at && !run.end);
  const pool = running.length > 0 ? running : runs;
  let latest: RunSummary | null = null;
  for (const run of pool) if (!latest || run.started_at >= latest.started_at) latest = run;
  return latest;
}

/** run の状態語。終わっていなければ running、終わっていれば結果（無ければ未確認）。 */
export function runStatus(run: RunSummary): string {
  if (!run.finished_at && !run.end) return "running";
  return run.outcome ?? "unknown";
}

type NextStep = { action: Action; label: string; target: "decision-panel" | "execution-panel" };

// 次の操作は「今できる操作」のうち人の判断を待つものだけ。button 自体は判断・実行 panel に置いたまま、
// header からはその panel へ移る link を出す（同じ名前の button を 2 つ作らない）。
const nextSteps: readonly NextStep[] = [
  { action: "approve", label: "承認を待っています", target: "decision-panel" },
  { action: "answer", label: "回答を待っています", target: "decision-panel" },
  { action: "phase_gate", label: "途中確認の決定を待っています", target: "execution-panel" },
  { action: "retry", label: "再試行できます", target: "decision-panel" },
];

export function availableNextSteps(actions: readonly Action[]): NextStep[] {
  return nextSteps.filter((step) => actions.includes(step.action));
}

/** 画面上部の header。h1 の下で、状態・現在の run・次の操作を tab に関係なく出す。 */
export function TaskDetailHeader({ taskId }: { taskId: string }) {
  const detail = useQuery(taskDetailQuery(taskId));
  if (!detail.data) return null;
  const { task } = detail.data;
  const run = currentRun(detail.data.runs);
  const steps = availableNextSteps(detail.data.actions);
  return (
    <section aria-label="タスクの現在" data-testid="task-header" className={card}>
      {/* 長い題は 3 行で省略し、全文は title 属性と下の概要で読める。 */}
      <p title={task.title} className="line-clamp-3 break-words text-body font-semibold text-foreground">
        {task.title}
      </p>
      <dl className="mt-3 flex min-w-0 flex-col gap-3 text-label md:flex-row md:flex-wrap md:gap-x-8">
        <div className="flex min-w-0 flex-col gap-1" data-testid="task-header-status">
          <dt className="font-medium text-muted-foreground">状態</dt>
          <dd>
            <StatusBadge status={task.status} />
          </dd>
        </div>
        <div className="flex min-w-0 flex-col gap-1" data-testid="task-header-run">
          <dt className="font-medium text-muted-foreground">現在の run</dt>
          <dd className="flex min-w-0 flex-wrap items-center gap-2">
            {run ? (
              <>
                <Link
                  to="/tasks/$id/runs/$runId"
                  params={{ id: task.id, runId: run.run_id }}
                  aria-label={`run ${run.run_id} を開く`}
                  className={`inline-flex min-h-11 min-w-0 max-w-full items-center ${textLink}`}
                >
                  <ShortId id={run.run_id} />
                </Link>
                <StatusBadge status={runStatus(run)} />
              </>
            ) : (
              <span className="text-muted-foreground">まだありません</span>
            )}
          </dd>
        </div>
        <div className="flex min-w-0 flex-col gap-1" data-testid="task-header-next">
          <dt className="font-medium text-muted-foreground">次の操作</dt>
          <dd>
            {steps.length > 0 ? (
              <ul className="flex flex-wrap gap-2">
                {steps.map((step) => (
                  <li key={step.action} data-action={step.action}>
                    <Link
                      to="/tasks/$id"
                      params={{ id: task.id }}
                      search={{ tab: undefined }}
                      hash={step.target}
                      className={buttonVariants({ variant: "secondary", size: "sm" })}
                    >
                      {step.label}
                    </Link>
                  </li>
                ))}
              </ul>
            ) : (
              <span className="text-muted-foreground">人の操作を待っていません</span>
            )}
          </dd>
        </div>
      </dl>
    </section>
  );
}

/** section を渡すとスマホ幅でその区画だけを出す（desktop は全部）。渡さなければ全部を出す。 */
export function OverviewView({ detail, section }: { detail: TaskDetail; section?: MobileSection }) {
  const task = detail.task;
  const facts: DataListItem[] = [
    { label: "状態", value: <span data-status={task.status}>{task.status}</span> },
    { label: "種別", value: task.kind },
  ];
  if (task.category) facts.push({ label: "カテゴリ", value: task.category });
  if (detail.priority_label) facts.push({ label: "優先度", value: detail.priority_label });
  if (task.assignee) facts.push({ label: "担当", value: task.assignee });
  if (task.genre) facts.push({ label: "分野", value: task.genre });
  facts.push(
    { label: "試行", value: `${task.attempts} / ${task.budget.max_retries}` },
    { label: "作成", value: task.created_at },
    { label: "更新", value: task.updated_at },
  );
  return (
    <div className="flex min-w-0 flex-col gap-4" data-testid="task-overview">
      <div className={mobileSectionClass("summary", section)}>
        <section className={card}>
          <h2 className={`${heading} break-words`}>{task.title}</h2>
          <DataList className="mt-2" items={facts} />
        </section>

        {task.objective ? (
          <section className={card}>
            <h2 className={heading}>目的</h2>
            <p className="mt-1 whitespace-pre-wrap break-words text-label">{task.objective}</p>
          </section>
        ) : null}

        {detail.failure ? (
          <section
            className="min-w-0 rounded-lg border border-destructive bg-danger p-4 text-danger-foreground"
            role="alert"
          >
            <h2 className="text-section font-semibold">失敗</h2>
            <p className="mt-1 break-words text-label">
              {detail.failure.class}: {detail.failure.reason}
            </p>
          </section>
        ) : null}

        {detail.latest_question ? (
          <section className="min-w-0 rounded-lg border border-border bg-warning p-4 text-warning-foreground">
            <h2 className="text-section font-semibold">質問待ち</h2>
            <p className="mt-1 whitespace-pre-wrap break-words text-label">{detail.latest_question}</p>
          </section>
        ) : null}
      </div>

      <div className={mobileSectionClass("tree", section)} id="task-tree">
        <TaskTree detail={detail} />
      </div>

      {/* スマホ幅では木の行から Drawer で開く（同じ id を 2 つ作らないよう inline は desktop だけ）。 */}
      <div className="hidden md:contents">
        <IntegrationRepairPanel view={detail.integration_repair} anchorId="integration-repair" />
      </div>

      <div className={mobileSectionClass("summary", section)}>
        <RelatedTasks title="依存" items={detail.dependencies} />
        <RelatedTasks title="依存元" items={detail.dependents} />

        {detail.criteria.length > 0 ? (
          <section className={card}>
            <h2 className={heading}>受け入れ条件（{detail.criteria.length}）</h2>
            <ul className="mt-2 flex flex-col gap-2">
              {detail.criteria.map((criterion) => (
                <li key={criterion.idx} className="min-w-0 text-label">
                  <span className="font-medium">{criterion.idx}: </span>
                  <span className="break-words">{criterion.text}</span>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {detail.runs.length > 0 ? (
          <section className={card}>
            <h2 className={heading}>run（{detail.runs.length}）</h2>
            <ul className="mt-2 flex flex-col gap-1">
              {detail.runs.map((run) => (
                <li key={run.run_id} className="flex min-w-0 flex-wrap items-center gap-x-2 text-label">
                  <Link
                    to="/tasks/$id/runs/$runId"
                    params={{ id: task.id, runId: run.run_id }}
                    className={`inline-flex min-h-11 min-w-0 max-w-full items-center ${textLink}`}
                  >
                    <ShortId id={run.run_id} />
                  </Link>
                  <StatusBadge status={runStatus(run)} />
                  <span className="min-w-0 break-words text-muted-foreground">
                    {run.adapter} / {run.model}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ) : null}
      </div>
    </div>
  );
}

// ---- 木 -------------------------------------------------------------------------------------------

type StageGroup = { key: string; title: string; units: WorkUnitView[] };

/** 計画の段（stages）に WU を振り分ける。段に属さない WU は最後の「段なし」に集める。 */
export function groupWorkUnits(execution: TaskExecutionView | undefined): StageGroup[] {
  const plan = execution?.plan;
  if (!plan) return [];
  const stages = plan.plan.stages ?? [];
  const groups: StageGroup[] = stages.map((stage) => ({ key: stage.key, title: stage.title, units: [] }));
  const loose: StageGroup = { key: "", title: "段なし", units: [] };
  for (const unit of [...plan.work_units].sort((a, b) => a.seq - b.seq)) {
    const group = groups.find((item) => item.key === (unit.phase ?? unit.spec.phase));
    (group ?? loose).units.push(unit);
  }
  return loose.units.length > 0 ? [...groups, loose] : groups;
}

const branch = "ml-2 flex min-w-0 flex-col gap-1 border-l border-border pl-3";
const nodeRow = "flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 py-1 text-label";
const kindLabel = "shrink-0 text-muted-foreground";

/** 親 task → この task →（段 → WU）・integration repair・子 task の木。 */
export function TaskTree({ detail }: { detail: TaskDetail }) {
  const task = detail.task;
  const execution = useQuery(executionQuery(task.id));
  const groups = groupWorkUnits(execution.data);
  const repair = detail.integration_repair;
  const repairDisplay = integrationRepairDisplay(repair);
  const repairUnit = repair?.work_unit_id ?? null;
  const repairInTree = groups.some((group) => group.units.some((unit) => unit.id === repairUnit));

  const repairNode = repairDisplay ? (
    <li className={nodeRow} data-testid="tree-integration-repair" data-state={repairDisplay.state}>
      <span className={kindLabel}>main から取り込み</span>
      <a href="#integration-repair" className={`hidden md:inline ${textLink}`}>
        integration repair
      </a>
      <Drawer
        title={repairDisplay.heading}
        description="main から取り込んだときの修復の状況"
        trigger={
          <button type="button" className={`inline-flex min-h-11 items-center md:hidden ${textLink}`}>
            integration repair
          </button>
        }
      >
        <IntegrationRepairPanel view={repair} />
      </Drawer>
      <Badge tone={integrationRepairTone(repairDisplay.tone)}>{repairDisplay.stateLabel}</Badge>
    </li>
  ) : null;

  const self = (
    <li className="min-w-0" data-testid="tree-self">
      <div className={nodeRow}>
        <span className={kindLabel}>{task.parent_id ? "この子 task" : "この task"}</span>
        <span title={task.title} className="line-clamp-2 min-w-0 break-words font-medium text-foreground">
          {task.title}
        </span>
        <StatusBadge status={task.status} />
      </div>
      <ul className={branch} aria-label="この task の中">
        {groups.map((group) => (
          <li key={group.key || "loose"} className="min-w-0" data-testid="tree-stage" data-stage={group.key}>
            <div className={nodeRow}>
              <span className={kindLabel}>段</span>
              <span className="min-w-0 break-words font-medium text-foreground">{group.title}</span>
              {group.key ? <ShortId id={group.key} /> : null}
            </div>
            <ul className={branch} aria-label={`段 ${group.title} の WU`}>
              {group.units.map((unit) => (
                <WorkUnitNode
                  key={unit.id}
                  taskId={task.id}
                  unit={unit}
                  repairNode={unit.id === repairUnit ? repairNode : null}
                />
              ))}
            </ul>
          </li>
        ))}
        {repairInTree ? null : repairNode}
        {detail.children.map((child) => (
          <li key={child.id} className={nodeRow} data-testid="tree-child">
            <span className={kindLabel}>子 task</span>
            <Link
              to="/tasks/$id"
              params={{ id: child.id }}
              className={`inline-flex min-h-11 min-w-0 items-center ${textLink}`}
            >
              {child.title}
            </Link>
            <StatusBadge status={child.status} />
            <ShortId id={child.id} />
          </li>
        ))}
      </ul>
    </li>
  );

  return (
    <section aria-labelledby="task-tree-title" data-testid="task-tree" className={card}>
      <h2 id="task-tree-title" className={heading}>
        木（親子 task・段・WU）
      </h2>
      {execution.isPending ? (
        <p role="status" className="mt-1 text-label text-muted-foreground">
          計画を読み込んでいます
        </p>
      ) : null}
      <ul className="mt-2 flex min-w-0 flex-col gap-1" aria-label="task の木">
        {task.parent_id ? (
          <li className="min-w-0" data-testid="tree-parent">
            <div className={nodeRow}>
              <span className={kindLabel}>親 task</span>
              <Link
                to="/tasks/$id"
                params={{ id: task.parent_id }}
                aria-label={`親 task ${task.parent_id} を開く`}
                className={`inline-flex min-h-11 min-w-0 max-w-full items-center ${textLink}`}
              >
                <ShortId id={task.parent_id} />
              </Link>
            </div>
            <ul className={branch}>{self}</ul>
          </li>
        ) : (
          self
        )}
      </ul>
    </section>
  );
}

function WorkUnitNode({ taskId, unit, repairNode }: { taskId: string; unit: WorkUnitView; repairNode: ReactNode }) {
  const runId = unit.running_run_id ?? unit.last_run_id ?? null;
  const title = unit.spec.title || unit.key;
  const childLink = unit.child_task_id ? (
    <Link
      to="/tasks/$id"
      params={{ id: unit.child_task_id }}
      aria-label={`WU ${unit.key} の子 task ${unit.child_task_id} を開く`}
      className={`inline-flex min-h-11 min-w-0 max-w-full items-center ${textLink}`}
    >
      <ShortId id={unit.child_task_id} />
    </Link>
  ) : null;
  const runLink = runId ? (
    <Link
      to="/tasks/$id/runs/$runId"
      params={{ id: taskId, runId }}
      aria-label={`WU ${unit.key} の run ${runId} を開く`}
      className={`inline-flex min-h-11 min-w-0 max-w-full items-center ${textLink}`}
    >
      <ShortId id={runId} />
    </Link>
  ) : null;
  return (
    <li className="min-w-0" data-testid="tree-work-unit" data-key={unit.key}>
      <div className={nodeRow}>
        <span className={kindLabel}>WU</span>
        <span className="min-w-0 break-words font-medium text-foreground">{title}</span>
        <StatusBadge status={unit.status} />
        {/* スマホ幅では key・子 task・run を Drawer に移し、行は題と状態だけにする。 */}
        <span className="hidden min-w-0 max-w-full flex-wrap items-center gap-x-2 md:inline-flex">
          <ShortId id={unit.key} />
          {childLink}
          {runLink}
        </span>
        <Drawer
          title={`WU ${title}`}
          description="WU の key・子 task・run"
          trigger={
            <button
              type="button"
              aria-label={`WU ${unit.key} の詳細`}
              className={`${buttonVariants({ variant: "ghost", size: "sm" })} md:hidden`}
            >
              詳細
            </button>
          }
        >
          <DataList
            items={[
              { label: "状態", value: <StatusBadge status={unit.status} /> },
              { label: "key", value: <span className="break-all font-mono">{unit.key}</span> },
              { label: "段", value: <span className="break-all">{unit.phase ?? unit.spec.phase ?? "なし"}</span> },
              { label: "ID", value: <span className="break-all font-mono">{unit.id}</span> },
              { label: "子 task", value: childLink ?? <span className="text-muted-foreground">なし</span> },
              { label: "run", value: runLink ?? <span className="text-muted-foreground">まだありません</span> },
            ]}
          />
        </Drawer>
      </div>
      {repairNode ? <ul className={branch}>{repairNode}</ul> : null}
    </li>
  );
}

function RelatedTasks({ title, items }: { title: string; items: TaskDetail["dependencies"] }) {
  if (items.length === 0) return null;
  return (
    <section className={card}>
      <h2 className={heading}>
        {title}（{items.length}）
      </h2>
      <ul className="mt-2 flex flex-col gap-1">
        {items.map((item) => (
          <li key={item.id} className="flex min-w-0 flex-wrap items-center gap-x-2 text-label">
            <Link
              to="/tasks/$id"
              params={{ id: item.id }}
              className={`inline-flex min-h-11 min-w-0 items-center ${textLink}`}
            >
              {item.title}
            </Link>
            <StatusBadge status={item.status} />
          </li>
        ))}
      </ul>
    </section>
  );
}
