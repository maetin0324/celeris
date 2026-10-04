import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../../api/client";
import type { ArtifactList } from "../../api/generated/types";
import { artifactListKey } from "../../api/realtime/invalidation-map";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ArtifactTable } from "./artifact-table";

// /tasks/:id?tab=artifacts（P3-13）。1 task の成果物。一覧と本文は /artifacts（P3-11）と同じ ArtifactTable（H8）。
export function taskArtifactsQuery(taskId: string) {
  return {
    queryKey: [...artifactListKey, { task: taskId }] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ArtifactList>(`/api/tasks/${encodeURIComponent(taskId)}/artifacts`, signal),
  };
}

export function TaskArtifactsPanel({ taskId }: { taskId: string }) {
  const list = useQuery(taskArtifactsQuery(taskId));
  const items = list.data?.items;
  return (
    <FetchFrame query={list} subject="成果物">
      {items ? (
        items.length === 0 ? (
          <div data-testid="task-artifacts-empty">
            <EmptyState message="このタスクの成果物はありません。" />
          </div>
        ) : (
          <ArtifactTable
            label={`タスク ${taskId} の成果物`}
            data-testid="task-artifacts"
            rows={items.map((view) => ({ taskId, marker: String(view.idx), view }))}
          />
        )
      ) : null}
    </FetchFrame>
  );
}
