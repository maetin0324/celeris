import { apiGet } from "../../api/client";
import type { ProjectDetail, ProjectList, ProjectOverview } from "../../api/generated/types";
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

// 案件の概要（ADR 2026-11-23-project-observability-publish D1、読み取り専用）。key は detail の下に置き、
// 案件の詳細を invalidate すると概要も読み直す（概要は detail と同じ行から導く）。
export function projectOverviewQuery(projectId: string) {
  return {
    queryKey: [...projectKeys.detail(projectId), "overview"] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<ProjectOverview>(`/api/projects/${encodeURIComponent(projectId)}/overview`, signal),
  };
}
