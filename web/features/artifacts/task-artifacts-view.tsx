import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../../api/client";
import type { ArtifactList } from "../../api/generated/types";
import { artifactListKey } from "../../api/realtime/invalidation-map";
import { ArtifactPreview } from "../../components/content/artifact-preview";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";

// /tasks/:id?tab=artifacts（P3-13）。1 task の成果物。本文は /artifacts（P3-11）と同じ ArtifactPreview（H8）。
export function taskArtifactsQuery(taskId: string) {
  return {
    queryKey: [...artifactListKey, { task: taskId }] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ArtifactList>(`/api/tasks/${encodeURIComponent(taskId)}/artifacts`, signal),
  };
}

export function TaskArtifactsPanel({ taskId }: { taskId: string }) {
  const list = useQuery(taskArtifactsQuery(taskId));
  return (
    <FetchFrame query={list}>
      {list.data ? (
        list.data.items.length === 0 ? (
          <p data-testid="task-artifacts-empty">このタスクの成果物はありません。</p>
        ) : (
          <ul data-testid="task-artifacts" className="min-w-0 space-y-2">
            {list.data.items.map((view) => (
              <li key={view.idx} className="min-w-0 break-words rounded border p-2" data-artifact={view.idx}>
                {view.exists && !view.forbidden ? (
                  <ArtifactPreview taskId={taskId} idx={view.idx} name={view.artifact.name} />
                ) : (
                  <p>
                    {view.artifact.name}（{view.forbidden ? "読めない場所です" : "file がありません"}）
                  </p>
                )}
              </li>
            ))}
          </ul>
        )
      ) : null}
    </FetchFrame>
  );
}
