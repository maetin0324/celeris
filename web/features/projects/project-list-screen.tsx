import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import type { Project, ProjectCreateBody, ProjectStatus } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { projectListQuery } from "./project-queries";

// R09 /projects（P4-01）: 一覧・アーカイブの絞り込み・作成。作成の成功で /projects/:id へ、422 は欄の横。

const control = "min-h-11 w-full rounded border border-neutral-400 bg-white p-2";

export const projectStatusLabels: Record<ProjectStatus, string> = {
  active: "進行中",
  done: "完了",
  proposed: "提案",
  paused: "一時停止",
  cancelled: "中止",
};

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
    <form onSubmit={submit} className="max-w-3xl space-y-4 rounded-lg border p-3" data-testid="project-create-form">
      <h2 className="text-lg font-semibold">案件を作る</h2>
      <div>
        <label htmlFor="project-title" className="block font-medium">
          案件名
        </label>
        <input
          id="project-title"
          className={control}
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          aria-describedby={invalid ? errorId : undefined}
        />
        {invalid && <ActionResultView result={result} fieldId={errorId} />}
      </div>
      <div>
        <label htmlFor="project-request" className="block font-medium">
          依頼
        </label>
        <textarea
          id="project-request"
          aria-label="依頼"
          className={control}
          rows={4}
          value={request}
          onChange={(event) => setRequest(event.target.value)}
          aria-describedby={invalid ? errorId : undefined}
        />
      </div>
      <div>
        <label htmlFor="project-workspace" className="block font-medium">
          作業場所（手元の path、任意）
        </label>
        <input
          id="project-workspace"
          className={control}
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

export function ProjectsListScreen({ archived }: { archived: boolean }) {
  const navigate = useNavigate();
  const list = useQuery(projectListQuery({ archived }));
  return (
    <ScreenFrame title="案件" route="/projects">
      <div className="min-w-0 space-y-4">
        <label className="flex min-h-11 items-center gap-2" data-testid="projects-filter">
          <input
            type="checkbox"
            className="h-11 w-11"
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
              <p data-testid="projects-empty">案件はありません。</p>
            ) : (
              <ul className="min-w-0 space-y-2" data-testid="projects-list">
                {list.data.items.map((project) => (
                  <li key={project.id} className="min-w-0 rounded border p-3" data-project-id={project.id}>
                    <Link className="font-medium underline break-words" to="/projects/$id" params={{ id: project.id }}>
                      {project.title}
                    </Link>
                    <p className="text-sm">
                      {projectStatusLabels[project.status]}
                      {project.archived_at ? " / アーカイブ済み" : ""}
                    </p>
                  </li>
                ))}
              </ul>
            )
          ) : null}
        </FetchFrame>
        <CreateForm />
      </div>
    </ScreenFrame>
  );
}
