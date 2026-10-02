import { apiGet } from "../../api/client";
import type { ArtifactList, ArtifactView, ProjectDetail, ProjectList } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";
import { artifactListKey } from "../../api/realtime/invalidation-map";

// /artifacts（P3-11、R20）。案件（?project=）を選ぶと、その案件のタスクの成果物を並べる。
// key は D6 の `['artifacts', filters]`（invalidation-map.ts の artifactListKey）。

export type ArtifactRow = { taskId: string; taskTitle: string; view: ArtifactView };

export function projectsQuery() {
  return {
    queryKey: projectKeys.list(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<ProjectList>("/api/projects", signal),
  };
}

export function artifactsQuery(projectId: string) {
  return {
    queryKey: [...artifactListKey, { project: projectId }] as const,
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<ArtifactRow[]> => {
      const detail = await apiGet<ProjectDetail>(`/api/projects/${encodeURIComponent(projectId)}`, signal);
      const lists = await Promise.all(
        detail.tasks.map((task) =>
          apiGet<ArtifactList>(`/api/tasks/${encodeURIComponent(task.id)}/artifacts`, signal).then((list) =>
            list.items.map((view) => ({ taskId: task.id, taskTitle: task.title, view })),
          ),
        ),
      );
      return lists.flat();
    },
  };
}
