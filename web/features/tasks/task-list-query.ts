import { apiGet } from "../../api/client";
import type { TaskList, TaskStatusCounts } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";

// /tasks の query（P3-05、ADR-0081 D5）。key は正規化した絞り込みを含むので、絞り込みを変えると新しい key。
// 絞り込み・並び・検索は search param からそのまま GET のクエリへ（GUI 側では再計算しない）。

export type TaskListFilters = {
  q?: string;
  status?: readonly string[];
  genre?: string;
  order?: string;
  limit?: string;
  run_state?: "runnable" | "waiting";
};

export function taskListPath(filters: TaskListFilters, cursor?: string | null): string {
  const params = new URLSearchParams();
  if (filters.q) params.set("q", filters.q);
  for (const status of filters.status ?? []) params.append("status", status);
  if (filters.genre) params.set("genre", filters.genre);
  if (filters.order) params.set("order", filters.order);
  if (filters.limit) params.set("limit", filters.limit);
  if (filters.run_state) params.set("run_state", filters.run_state);
  if (cursor) params.set("cursor", cursor);
  // 取り込み状態（integrations / promotion）は一覧が表示するので明示して頼む（ADR 2026-10-21-task-list-opt-in-integrations D1）。
  params.set("with", "integrations");
  return params.toString() === "" ? "/api/tasks" : `/api/tasks?${params.toString()}`;
}

export function taskListQueryKey(filters: TaskListFilters) {
  return taskKeys.list({
    q: filters.q,
    status: filters.status,
    genre: filters.genre,
    order: filters.order,
    limit: filters.limit,
    run_state: filters.run_state,
    with: "integrations",
  });
}

export function taskListQuery(filters: TaskListFilters) {
  return {
    queryKey: taskListQueryKey(filters),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<TaskList>(taskListPath(filters), signal),
  };
}

export function taskCountsQuery() {
  return {
    queryKey: taskKeys.counts(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<TaskStatusCounts>("/api/tasks/counts", signal),
  };
}
