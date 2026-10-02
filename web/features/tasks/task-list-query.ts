import { apiGet } from "../../api/client";
import type { TaskList } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";

// /tasks の query（P3-05、ADR-0081 D5）。key は正規化した絞り込みを含むので、絞り込みを変えると新しい key。
// 絞り込み・並び・検索は search param からそのまま GET のクエリへ（GUI 側では再計算しない）。

export type TaskListFilters = {
  q?: string;
  status?: readonly string[];
  genre?: string;
  order?: string;
  limit?: string;
};

export function taskListPath(filters: TaskListFilters, cursor?: string | null): string {
  const params = new URLSearchParams();
  if (filters.q) params.set("q", filters.q);
  for (const status of filters.status ?? []) params.append("status", status);
  if (filters.genre) params.set("genre", filters.genre);
  if (filters.order) params.set("order", filters.order);
  if (filters.limit) params.set("limit", filters.limit);
  if (cursor) params.set("cursor", cursor);
  return params.toString() === "" ? "/api/tasks" : `/api/tasks?${params.toString()}`;
}

export function taskListQueryKey(filters: TaskListFilters) {
  return taskKeys.list({
    q: filters.q,
    status: filters.status,
    genre: filters.genre,
    order: filters.order,
    limit: filters.limit,
  });
}

export function taskListQuery(filters: TaskListFilters) {
  return {
    queryKey: taskListQueryKey(filters),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<TaskList>(taskListPath(filters), signal),
  };
}
