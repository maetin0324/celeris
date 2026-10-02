import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import type { ProjectDetail } from "../../api/generated/types";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { TaskArtifactsPanel } from "../artifacts/task-artifacts-view";
import { projectStatusLabels } from "./project-list-screen";
import { projectDetailQuery } from "./project-queries";
import { dagLayers, type WorkTreeNode, workTree } from "./project-structure";

// R10 /projects/:id の表示（P4-02）: 概要・計画の DAG・仕事の木・成果物の一覧。
// DAG と木は枠の中でスクロールし、ページは横に溢れない。操作（P4-03〜P4-05）は `ops` の差し込み口に載せる。

export function ProjectSection({ title, testId, children }: { title: string; testId: string; children: ReactNode }) {
  return (
    <section className="min-w-0 space-y-2 rounded-lg border border-neutral-300 p-3" data-testid={testId}>
      <h2 className="text-lg font-semibold">{title}</h2>
      {children}
    </section>
  );
}

function Overview({ detail }: { detail: ProjectDetail }) {
  const { project, root_totals: totals } = detail;
  return (
    <ProjectSection title="概要" testId="project-overview">
      <p>
        <span data-testid="project-status">{projectStatusLabels[project.status]}</span>
        {project.archived_at ? " / アーカイブ済み" : ""}
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
            .map(([status, count]) => ` / ${status} ${count}`)
            .join("")}
        </p>
      )}
      <Markdown source={project.secretary_summary || project.request} />
    </ProjectSection>
  );
}

function PlanDag({ detail }: { detail: ProjectDetail }) {
  const nodes = detail.project_plan?.nodes ?? [];
  if (nodes.length === 0) return <p>計画はまだありません。</p>;
  return (
    <div
      className="max-w-full overflow-auto rounded border"
      data-testid="project-dag-frame"
      style={{ maxHeight: "70vh" }}
    >
      <ol className="flex w-max gap-4 p-3">
        {dagLayers(nodes).map((layer, index) => (
          <li key={layer[0]?.key ?? index} className="w-56 shrink-0 space-y-2">
            <p className="text-sm">段 {index + 1}</p>
            <ul className="space-y-2">
              {layer.map((node) => (
                <li key={node.key} className="rounded border p-2 break-words" data-dag-node={node.key}>
                  <Link className="font-medium underline" to="/tasks/$id" params={{ id: node.task_id }}>
                    {node.title}
                  </Link>
                  <p className="text-sm">
                    {node.milestone_status ?? node.task_status ?? "—"} / 子 {node.children_done}/{node.children_total}
                  </p>
                  {node.depends_on.length > 0 && <p className="text-sm">依存: {node.depends_on.join(", ")}</p>}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ol>
    </div>
  );
}

function TreeItems({ nodes }: { nodes: readonly WorkTreeNode[] }) {
  return (
    <ul className="space-y-1 border-l pl-4">
      {nodes.map((node) => (
        <li key={node.task.id} data-tree-task={node.task.id}>
          <span className="whitespace-nowrap">
            <Link className="underline" to="/tasks/$id" params={{ id: node.task.id }}>
              {node.task.title}
            </Link>{" "}
            <span className="text-sm">{node.task.status}</span>
          </span>
          {node.children.length > 0 && <TreeItems nodes={node.children} />}
        </li>
      ))}
    </ul>
  );
}

function WorkTree({ detail }: { detail: ProjectDetail }) {
  if (detail.tasks.length === 0) return <p>仕事はまだありません。</p>;
  return (
    <div
      className="max-w-full overflow-auto rounded border p-3"
      data-testid="project-tree-frame"
      style={{ maxHeight: "70vh" }}
    >
      <div className="w-max">
        <TreeItems nodes={workTree(detail.tasks)} />
      </div>
    </div>
  );
}

function RootArtifacts({ detail }: { detail: ProjectDetail }) {
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const roots = detail.tasks.filter((task) => task.is_root_task || !task.parent_id);
  if (roots.length === 0) return <p>成果物はありません。</p>;
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
            <p className="text-xl font-semibold break-words">{detail.data.project.title}</p>
            <nav aria-label="案件の頁" className="flex flex-wrap gap-3">
              <Link className="underline" to="/projects/$id/docs" params={{ id: projectId }}>
                文書
              </Link>
              <Link className="underline" to="/board">
                ボード
              </Link>
            </nav>
            <Overview detail={detail.data} />
            {ops?.(detail.data)}
            <ProjectSection title="計画の DAG" testId="project-dag">
              <PlanDag detail={detail.data} />
            </ProjectSection>
            <ProjectSection title="仕事の木" testId="project-tree">
              <WorkTree detail={detail.data} />
            </ProjectSection>
            <ProjectSection title="成果物" testId="project-artifacts-section">
              <RootArtifacts detail={detail.data} />
            </ProjectSection>
          </div>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
