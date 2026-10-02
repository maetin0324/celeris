import { apiGet } from "../../api/client";
import type { ProjectDetail, ProjectList } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";

// /projects と /projects/:id の query（P4-01・P4-02、ADR-0081 D5）。
// アーカイブの出し入れは celeris が行う（`archived=1`、ADR-0044 D6）。GUI 側で archived_at を見て絞り直さない。

export type ProjectListFilters = { archived?: boolean };

export function projectListPath(filters: ProjectListFilters): string {
  return filters.archived ? "/api/projects?archived=1" : "/api/projects";
}

export function projectListQuery(filters: ProjectListFilters) {
  return {
    queryKey: projectKeys.list({ archived: filters.archived ? "1" : undefined }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<ProjectList>(projectListPath(filters), signal),
  };
}

export function projectDetailQuery(projectId: string) {
  return {
    queryKey: projectKeys.detail(projectId),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ProjectDetail>(`/api/projects/${encodeURIComponent(projectId)}`, signal),
  };
}
