import { useQueries, useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import type { Project, ProjectCreateBody, ProjectStatus } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute, formatRelative } from "../../lib/time";
import { projectDetailQuery, projectListQuery } from "./project-queries";
import { milestoneProgress, pendingByProject } from "./project-structure";

// R09 /projects（P4-01）: 一覧・アーカイブの絞り込み・作成。作成の成功で /projects/:id へ、422 は欄の横。
// 一覧は card にせず table（状態・途中目標の進み・判断待ちの数・最終更新）で 1 行 1 案件の密度を保つ。

export const inputClass = "min-h-11 w-full min-w-0 rounded-md border bg-surface px-2 py-2 text-body text-foreground";

export const projectStatusLabels: Record<ProjectStatus, string> = {
  active: "進行中",
  done: "完了",
  proposed: "提案",
  paused: "一時停止",
  cancelled: "中止",
};

export const projectStatusTone: Record<ProjectStatus, BadgeTone> = {
  active: "running",
  done: "success",
  proposed: "info",
  paused: "warning",
  cancelled: "neutral",
};

export function ProjectStatusBadge({ status }: { status: ProjectStatus }) {
  return (
    <Badge tone={projectStatusTone[status]} data-status={status}>
      {projectStatusLabels[status] ?? status}
    </Badge>
  );
}

function createdId(response: unknown): string | null {
  if (!response || typeof response !== "object") return null;
  const id = (response as Partial<Project>).id;
  return typeof id === "string" && id !== "" ? id : null;
}

function CreateForm() {
  const navigate = useNavigate();
  const sender = useActionResult(projectKeys.lists());
  const [title, setTitle] = useState("");
  const [request, setRequest] = useState("");
  const [path, setPath] = useState("");
  const result = sender.results["create-project"];
  const errorId = "project-create-error";
  const invalid = result?.status === 422;
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const body: ProjectCreateBody = { title, request };
    if (path.trim()) body.workspace = { kind: "local", path: path.trim() };
    const [outcome] = await sender.run([{ id: "create-project", path: "/api/projects", body }]);
    if (outcome?.ok) {
      const projectId = createdId(outcome.response);
      if (projectId) void navigate({ to: "/projects/$id", params: { id: projectId } });
    }
  }
  return (
    <form
      onSubmit={submit}
      className="max-w-form space-y-4 rounded-lg border border-border bg-surface p-4"
      data-testid="project-create-form"
    >
      <h2 className="text-section font-semibold">案件を作る</h2>
      <div>
        <label htmlFor="project-title" className="block text-label font-medium">
          案件名
        </label>
        <input
          id="project-title"
          className={inputClass}
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          aria-describedby={invalid ? errorId : undefined}
        />
        {invalid && <ActionResultView result={result} fieldId={errorId} />}
      </div>
      <div>
        <label htmlFor="project-request" className="block text-label font-medium">
          依頼
        </label>
        <textarea
          id="project-request"
          aria-label="依頼"
          className={inputClass}
          rows={4}
          value={request}
          onChange={(event) => setRequest(event.target.value)}
          aria-describedby={invalid ? errorId : undefined}
        />
      </div>
      <div>
        <label htmlFor="project-workspace" className="block text-label font-medium">
          作業場所（手元の path、任意）
        </label>
        <input
          id="project-workspace"
          className={inputClass}
          value={path}
          onChange={(event) => setPath(event.target.value)}
        />
      </div>
      <Button type="submit" disabled={sender.pending}>
        案件を作成
      </Button>
      {result && !invalid && <ActionResultView result={result} />}
    </form>
  );
}

function ProjectTable({ projects }: { projects: readonly Project[] }) {
  // 途中目標の進みは詳細の query から出す（詳細画面と cache を共有する）。判断待ちは受信箱の 1 本の query を数える。
  const details = useQueries({ queries: projects.map((project) => ({ ...projectDetailQuery(project.id) })) });
  const inbox = useQuery(inboxItemsQuery());
  const pending = inbox.data ? pendingByProject(inbox.data.items) : null;
  return (
    <Table aria-label="案件の一覧" data-testid="projects-list">
      <TableHeader>
        <TableRow>
          <TableHead>案件</TableHead>
          <TableHead>状態</TableHead>
          <TableHead>途中目標</TableHead>
          <TableHead>判断待ち</TableHead>
          <TableHead>最終更新</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {projects.map((project, index) => {
          const progress = details[index]?.data ? milestoneProgress(details[index].data) : null;
          const waiting = pending?.get(project.id) ?? 0;
          return (
            <TableRow key={project.id} data-project-id={project.id}>
              <TableCell className="min-w-40">
                <Link
                  className="inline-flex min-h-11 min-w-11 items-center font-medium text-primary underline break-words"
                  to="/projects/$id"
                  params={{ id: project.id }}
                >
                  {project.title}
                </Link>
              </TableCell>
              <TableCell>
                <div className="flex flex-col gap-1">
                  <ProjectStatusBadge status={project.status} />
                  {project.archived_at ? <span className="text-muted-foreground">アーカイブ済み</span> : null}
                </div>
              </TableCell>
              <TableCell className="whitespace-nowrap tabular-nums" data-testid="project-milestones">
                {progress ? `${progress.reached} / ${progress.total} 達成` : "—"}
              </TableCell>
              <TableCell className="whitespace-nowrap tabular-nums" data-testid="project-pending">
                {pending === null ? (
                  "—"
                ) : waiting > 0 ? (
                  <Link className="inline-flex min-h-11 min-w-11 items-center text-primary underline" to="/inbox">
                    {waiting} 件
                  </Link>
                ) : (
                  <span className="text-muted-foreground">0 件</span>
                )}
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <time dateTime={project.updated_at} title={formatAbsolute(project.updated_at)}>
                  {formatRelative(project.updated_at)}
                </time>
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

export function ProjectsListScreen({ archived }: { archived: boolean }) {
  const navigate = useNavigate();
  const list = useQuery(projectListQuery({ archived }));
  return (
    <ScreenFrame title="案件" route="/projects">
      <div className="min-w-0 space-y-4">
        <label className="flex min-h-11 w-fit items-center gap-2 text-label" data-testid="projects-filter">
          <input
            type="checkbox"
            className="h-11 w-11 accent-primary"
            checked={archived}
            onChange={(event) =>
              void navigate({ to: "/projects", search: event.target.checked ? { archived: true } : {} })
            }
          />
          アーカイブした案件も出す
        </label>
        <FetchFrame query={list}>
          {list.data ? (
            list.data.items.length === 0 ? (
              <p className="text-muted-foreground" data-testid="projects-empty">
                案件はありません。下の「案件を作る」から始めます。
              </p>
            ) : (
              <ProjectTable projects={list.data.items} />
            )
          ) : null}
        </FetchFrame>
        <CreateForm />
      </div>
    </ScreenFrame>
  );
}
