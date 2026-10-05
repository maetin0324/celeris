import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import type { MilestoneStatus, ProjectDetail } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { StatusBadge, statusView } from "../../components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { TaskArtifactsPanel } from "../artifacts/task-artifacts-view";
import { ProjectStatusBadge } from "./project-list-screen";
import { projectDetailQuery } from "./project-queries";
import { dagLayers, flattenTree, milestoneGroups, pendingByTask } from "./project-structure";

// R10 /projects/:id の表示（P4-02）: 概要・途中目標ごとの仕事の木・計画の DAG・成果物の一覧。
// 案件 → 途中目標 → task（子 task の木）を 1 つの表で読ませる。途中目標は表の区切り行、task は字下げした行。
// DAG と木は枠の中でスクロールし、ページは横に溢れない。操作（P4-03〜P4-05）は `ops` の差し込み口に載せる。

export function ProjectSection({
  title,
  testId,
  description,
  children,
}: {
  title: string;
  testId: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section
      className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4"
      data-testid={testId}
      aria-label={title}
    >
      <div>
        <h2 className="text-section font-semibold">{title}</h2>
        {description ? <p className="mt-1 text-label text-muted-foreground">{description}</p> : null}
      </div>
      {children}
    </section>
  );
}

const milestoneLabels: Record<MilestoneStatus, string> = {
  proposed: "提案",
  approved: "承認済み",
  in_progress: "進行中",
  reached: "達成",
  redesigned: "再設計",
  paused: "一時停止",
  cancelled: "中止",
};

const milestoneTones: Record<MilestoneStatus, BadgeTone> = {
  proposed: "info",
  approved: "info",
  in_progress: "running",
  reached: "success",
  redesigned: "neutral",
  paused: "warning",
  cancelled: "neutral",
};

export function MilestoneBadge({ status }: { status: MilestoneStatus }) {
  return (
    <Badge tone={milestoneTones[status] ?? "neutral"} data-status={status} className="whitespace-nowrap break-normal">
      途中目標: {milestoneLabels[status] ?? status}
    </Badge>
  );
}

// 字下げは token の余白で段を付ける（任意値を使わない）。深い段は最後の幅で止め、深さは文字でも出す。
const indent = ["ps-2", "ps-6", "ps-10", "ps-14", "ps-18", "ps-22", "ps-26", "ps-30", "ps-34"] as const;

function Overview({ detail }: { detail: ProjectDetail }) {
  const { project, root_totals: totals } = detail;
  return (
    <ProjectSection title="概要" testId="project-overview">
      <p className="flex flex-wrap items-center gap-2">
        <span data-testid="project-status">
          <ProjectStatusBadge status={project.status} />
        </span>
        {project.archived_at ? <span className="text-muted-foreground">アーカイブ済み</span> : null}
      </p>
      <p className="break-words">
        作業場所:{" "}
        {project.workspace
          ? project.workspace.kind === "local"
            ? project.workspace.path
            : `${project.workspace.cluster}:${project.workspace.path ?? ""}`
          : "未設定"}
      </p>
      {totals && (
        <p data-testid="project-totals">
          根の task {totals.root_tasks} 件
          {Object.entries(totals.by_status ?? {})
            .map(([status, count]) => ` / ${statusView(status).label} ${count}`)
            .join("")}
        </p>
      )}
      <Markdown source={project.secretary_summary || project.request} />
    </ProjectSection>
  );
}

function PlanDag({ detail }: { detail: ProjectDetail }) {
  const nodes = detail.project_plan?.nodes ?? [];
  const titles = new Map(nodes.map((node) => [node.key, node.title]));
  if (nodes.length === 0) return <p className="text-muted-foreground">計画はまだありません。</p>;
  return (
    <div
      className="max-h-screen max-w-full overflow-auto rounded-md border border-border"
      data-testid="project-dag-frame"
    >
      <ol className="flex w-max gap-4 p-3">
        {dagLayers(nodes).map((layer, index) => (
          <li key={layer[0]?.key ?? index} className="w-56 shrink-0 space-y-2">
            <p className="text-label text-muted-foreground">段 {index + 1}</p>
            <ul className="space-y-2">
              {layer.map((node) => (
                <li
                  key={node.key}
                  className="space-y-1 rounded-md border border-border p-2 break-words"
                  data-dag-node={node.key}
                >
                  <Link className="font-medium text-primary underline" to="/tasks/$id" params={{ id: node.task_id }}>
                    {node.title}
                  </Link>
                  <p className="flex flex-wrap items-center gap-1 text-label">
                    {node.milestone_status ? (
                      <MilestoneBadge status={node.milestone_status} />
                    ) : node.task_status ? (
                      <StatusBadge status={node.task_status} />
                    ) : null}
                    <span className="text-muted-foreground tabular-nums">
                      子 {node.children_done}/{node.children_total}
                    </span>
                  </p>
                  {node.depends_on.length > 0 && (
                    <p className="text-label text-muted-foreground">
                      依存: {node.depends_on.map((key) => titles.get(key) ?? key).join("、")}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ol>
    </div>
  );
}

function WorkTree({ detail }: { detail: ProjectDetail }) {
  const inbox = useQuery(inboxItemsQuery({ project: detail.project.id }));
  const pending = inbox.data ? pendingByTask(inbox.data.items) : new Map<string, number>();
  const groups = milestoneGroups(detail);
  if (groups.length === 0) return <p className="text-muted-foreground">途中目標も仕事もまだありません。</p>;
  return (
    <section
      aria-label="途中目標と仕事の表"
      // biome-ignore lint/a11y/noNoninteractiveTabindex: 横スクロールの枠をキーボードで動かせるようにする
      tabIndex={0}
      className="max-w-full overflow-auto rounded-md border border-border"
      data-testid="project-tree-frame"
    >
      <Table wrapperClassName="overflow-x-visible">
        {/* 640px 未満は見出しの行を隠し、各行を「題名」の下に「状態: …・判断待ち: …」と積む（右で切れない）。 */}
        <TableHeader className="max-sm:sr-only">
          <TableRow>
            <TableHead>仕事</TableHead>
            <TableHead>状態</TableHead>
            <TableHead>判断待ち</TableHead>
          </TableRow>
        </TableHeader>
        {groups.map((group, index) => {
          const rows = flattenTree(group.roots);
          return (
            <TableBody key={group.milestoneId ?? "none"} data-milestone={group.milestoneId ?? "none"}>
              <TableRow className="bg-muted hover:bg-muted">
                <TableHead scope="colgroup" colSpan={3} className="text-foreground">
                  <span className="flex flex-wrap items-center gap-2 whitespace-normal">
                    <span className="font-semibold">
                      {group.milestoneId ? `途中目標 ${index + 1}: ` : ""}
                      {group.title}
                    </span>
                    {group.milestoneStatus ? <MilestoneBadge status={group.milestoneStatus} /> : null}
                    {group.progress ? (
                      <span className="font-normal text-muted-foreground tabular-nums">
                        子 {group.progress.done}/{group.progress.total} 完了
                      </span>
                    ) : null}
                  </span>
                </TableHead>
              </TableRow>
              {rows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={3} className="text-muted-foreground">
                    仕事はまだありません。
                  </TableCell>
                </TableRow>
              ) : (
                rows.map(({ task, depth }) => {
                  const waiting = pending.get(task.id) ?? 0;
                  return (
                    <TableRow
                      key={task.id}
                      data-tree-task={task.id}
                      data-depth={depth}
                      className="max-sm:flex max-sm:flex-wrap max-sm:items-center"
                    >
                      <TableCell
                        className={`min-w-48 break-words max-sm:w-full max-sm:min-w-0 max-sm:pb-0 ${indent[Math.min(depth, indent.length - 1)]}`}
                      >
                        {depth > 0 ? (
                          <span aria-hidden="true" className="me-1 text-muted-foreground">
                            └
                          </span>
                        ) : null}
                        <span className="sr-only">{depth > 0 ? `${depth} 段下の子: ` : "根: "}</span>
                        <Link
                          className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
                          to="/tasks/$id"
                          params={{ id: task.id }}
                        >
                          {task.title}
                        </Link>
                      </TableCell>
                      <TableCell
                        className={`whitespace-nowrap max-sm:flex max-sm:items-center max-sm:gap-1 ${indent[Math.min(depth, indent.length - 1)]} sm:ps-2`}
                      >
                        <span className="text-label text-muted-foreground sm:hidden">状態:</span>
                        <StatusBadge status={task.status} className="whitespace-nowrap break-normal" />
                      </TableCell>
                      <TableCell className="whitespace-nowrap max-sm:flex max-sm:items-center max-sm:gap-1">
                        {/* 判断待ちのある行は link の文が「判断待ち N 件」なので、見出しは無い行（—）にだけ付ける。 */}
                        {waiting > 0 ? null : (
                          <span className="text-label text-muted-foreground sm:hidden">判断待ち:</span>
                        )}
                        {waiting > 0 ? (
                          <Link
                            className="inline-flex min-h-11 min-w-11 items-center font-medium text-primary underline"
                            to="/inbox"
                            data-testid="tree-task-pending"
                          >
                            判断待ち {waiting} 件
                          </Link>
                        ) : (
                          <span className="text-muted-foreground">—</span>
                        )}
                      </TableCell>
                    </TableRow>
                  );
                })
              )}
            </TableBody>
          );
        })}
      </Table>
    </section>
  );
}

function RootArtifacts({ detail }: { detail: ProjectDetail }) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const roots = detail.tasks.filter((task) => task.is_root_task || !task.parent_id);
  if (roots.length === 0) return <p className="text-muted-foreground">成果物はありません。</p>;
  // 成果物は task ごとに開いたときだけ取る（一覧で全 task へ要求を出さない）。
  return (
    <ul className="min-w-0 space-y-2" data-testid="project-artifacts">
      {roots.map((task) => (
        <li key={task.id} className="min-w-0">
          <details onToggle={(event) => setOpen((current) => ({ ...current, [task.id]: event.currentTarget.open }))}>
            <summary className="min-h-11 cursor-pointer break-words py-2">{task.title} の成果物</summary>
            {open[task.id] && <TaskArtifactsPanel taskId={task.id} />}
          </details>
        </li>
      ))}
    </ul>
  );
}

export function ProjectDetailScreen({
  projectId,
  ops,
}: {
  projectId: string;
  /** P4-03〜P4-05 の操作の差し込み口。 */
  ops?: (detail: ProjectDetail) => ReactNode;
}) {
  const detail = useQuery(projectDetailQuery(projectId));
  return (
    <ScreenFrame title={`案件の詳細 ${projectId}`} route="/projects/:id">
      <FetchFrame query={detail}>
        {detail.data ? (
          <div className="min-w-0 space-y-4" data-project={detail.data.project.id}>
            <p className="text-title font-semibold break-words" data-testid="project-title">
              {detail.data.project.title}
            </p>
            <nav aria-label="案件の頁" className="flex flex-wrap gap-2">
              <Link className="inline-flex min-h-11 min-w-11 items-center text-primary underline" to="/projects">
                案件の一覧
              </Link>
              <Link
                className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
                to="/projects/$id/docs"
                params={{ id: projectId }}
              >
                文書
              </Link>
              <Link
                className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
                to="/board"
                search={{ project: projectId }}
              >
                ボード
              </Link>
            </nav>
            <Overview detail={detail.data} />
            <ProjectSection
              title="途中目標と仕事"
              testId="project-tree"
              description="途中目標ごとに、根の仕事と子の仕事を字下げで並べます。判断待ちは受信箱へ。"
            >
              <WorkTree detail={detail.data} />
            </ProjectSection>
            <ProjectSection title="計画の DAG" testId="project-dag" description="途中目標の依存の順（左から段）。">
              <PlanDag detail={detail.data} />
            </ProjectSection>
            {ops?.(detail.data)}
            <ProjectSection title="成果物" testId="project-artifacts-section">
              <RootArtifacts detail={detail.data} />
            </ProjectSection>
          </div>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
