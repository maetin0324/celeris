import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArtifactPreview } from "../../components/content/artifact-preview";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { buttonClassName } from "../../components/ui/button";
import { type ArtifactRow, artifactsQuery, projectsQuery } from "./artifacts-query";

// /artifacts（P3-11、R20）。絞り込みは GET の form（?project=）。本文は受信箱（P3-03）と同じ ArtifactPreview で開く。
// HTML は同一オリジンで描画・実行しない（H8）: ArtifactPreview は Markdown 以外を download リンクにする。
export function ArtifactsScreen({ project }: { project?: string }) {
  const projects = useQuery(projectsQuery());
  return (
    <ScreenFrame title="成果物" route="/artifacts">
      <form method="get" action="/artifacts" className="flex flex-wrap items-end gap-2" data-testid="artifacts-filter">
        <label className="flex flex-col text-sm">
          案件
          <select name="project" defaultValue={project ?? ""} className="min-h-11 rounded border px-2" key={project}>
            <option value="">案件を選ぶ</option>
            {project && !projects.data?.items.some((item) => item.id === project) ? (
              <option value={project}>{project}</option>
            ) : null}
            {projects.data?.items.map((item) => (
              <option key={item.id} value={item.id}>
                {item.id}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" className={buttonClassName}>
          絞り込む
        </button>
      </form>
      {project ? <ArtifactsList projectId={project} /> : <p>案件を選ぶと、その案件のタスクの成果物を表示します。</p>}
    </ScreenFrame>
  );
}

function ArtifactsList({ projectId }: { projectId: string }) {
  const rows = useQuery(artifactsQuery(projectId));
  return (
    <FetchFrame query={rows}>
      {rows.data ? (
        rows.data.length === 0 ? (
          <p data-testid="artifacts-empty">この案件の成果物はありません。</p>
        ) : (
          <ul data-testid="artifacts-list" className="min-w-0 space-y-2">
            {rows.data.map((row) => (
              <ArtifactItem key={`${row.taskId}-${row.view.idx}`} row={row} />
            ))}
          </ul>
        )
      ) : null}
    </FetchFrame>
  );
}

function ArtifactItem({ row }: { row: ArtifactRow }) {
  const { view } = row;
  return (
    <li className="min-w-0 break-words rounded border p-2" data-artifact={`${row.taskId}/${view.idx}`}>
      <p className="text-sm">
        <Link to="/tasks/$id" params={{ id: row.taskId }} className="underline">
          {row.taskId}
        </Link>{" "}
        {row.taskTitle}
      </p>
      {view.exists && !view.forbidden ? (
        <ArtifactPreview taskId={row.taskId} idx={view.idx} name={view.artifact.name} />
      ) : (
        <p>
          {view.artifact.name}（{view.forbidden ? "読めない場所です" : "file がありません"}）
        </p>
      )}
    </li>
  );
}
