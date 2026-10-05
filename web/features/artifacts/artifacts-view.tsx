import { useQuery } from "@tanstack/react-query";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { ArtifactTable } from "./artifact-table";
import { artifactsQuery, projectsQuery } from "./artifacts-query";

// /artifacts（P3-11、R20）。絞り込みは GET の form（?project=）。本文は受信箱（P3-03）と同じ ArtifactPreview で開く。
// HTML は同一オリジンで描画・実行しない（H8）: ArtifactPreview は Markdown 以外を download リンクにする。
export function ArtifactsScreen({ project }: { project?: string }) {
  const projects = useQuery(projectsQuery());
  const recentProject = projects.data?.items.reduce<(typeof projects.data.items)[number] | undefined>(
    (latest, item) =>
      !latest || (item.updated_at ?? item.created_at ?? "") > (latest.updated_at ?? latest.created_at ?? "")
        ? item
        : latest,
    undefined,
  );
  const shownProject = project ?? recentProject?.id;
  return (
    <ScreenFrame title="成果物" route="/artifacts">
      <form
        method="get"
        action="/artifacts"
        className="flex min-w-0 flex-wrap items-end gap-2"
        data-testid="artifacts-filter"
      >
        <label className="flex min-w-0 flex-col gap-1 text-label text-muted-foreground">
          案件
          <select
            name="project"
            defaultValue={shownProject ?? ""}
            // 案件一覧が届いたら選び直す（届く前の仮の option が消えて選択が外れるのを防ぐ）。
            key={`${shownProject ?? ""}:${projects.data ? "loaded" : "pending"}`}
            className="min-h-11 min-w-0 max-w-full rounded-md border border-input bg-background px-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
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
        <Button type="submit">絞り込む</Button>
      </form>
      {projects.isError && projects.data === undefined ? (
        <p role="status" className="text-label text-muted-foreground">
          案件の一覧を取得できませんでした。案件 id を URL の ?project= で指定しても開けます。
        </p>
      ) : null}
      {!project && recentProject ? (
        <p className="text-label text-muted-foreground">
          最近更新された案件「{recentProject.title || recentProject.id}
          」の成果物を表示しています。別の案件は上から選べます。
        </p>
      ) : null}
      {shownProject ? (
        <ArtifactsList projectId={shownProject} />
      ) : (
        <p className="text-body text-muted-foreground">
          {projects.isLoading ? "案件を読み込んでいます。" : "案件がありません。案件を作成すると成果物を確認できます。"}
        </p>
      )}
    </ScreenFrame>
  );
}

function ArtifactsList({ projectId }: { projectId: string }) {
  const rows = useQuery(artifactsQuery(projectId));
  return (
    <FetchFrame query={rows} subject="成果物">
      {rows.data?.length === 0 ? (
        <div data-testid="artifacts-empty">
          <EmptyState message="この案件の成果物はありません。" />
        </div>
      ) : rows.data ? (
        <ArtifactTable
          label={`成果物一覧: ${projectId}`}
          data-testid="artifacts-list"
          rows={[...rows.data]
            .sort((a, b) => b.view.ts.localeCompare(a.view.ts))
            .map((row) => ({
              taskId: row.taskId,
              taskTitle: row.taskTitle,
              marker: `${row.taskId}/${row.view.idx}`,
              view: row.view,
            }))}
        />
      ) : null}
    </FetchFrame>
  );
}
