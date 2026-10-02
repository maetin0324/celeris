import { apiGet } from "../../api/client";
import type { ChangeDiffView, ChangesView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";

// /tasks/:id/changes と ?tab=changes の query（P3-13、R25）。key は taskKeys.changes（invalidation-map の changes 集合）。
// 差分は選んだファイルだけを取りに行く（repo と path は対）。

export function taskChangesPath(taskId: string): string {
  return `/api/tasks/${encodeURIComponent(taskId)}/changes`;
}

export function taskRepoChangesPath(taskId: string, repo: string): string {
  return `${taskChangesPath(taskId)}/${encodeURIComponent(repo)}`;
}

export function taskChangesQuery(taskId: string) {
  return {
    queryKey: taskKeys.changes(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<ChangesView>(taskChangesPath(taskId), signal),
  };
}

export function taskChangeDiffQuery(taskId: string, repo: string, path: string) {
  return {
    queryKey: [...taskKeys.changes(taskId), "diff", repo, path] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ChangeDiffView>(
        `${taskRepoChangesPath(taskId, repo)}/diff?${new URLSearchParams({ path }).toString()}`,
        signal,
      ),
  };
}
