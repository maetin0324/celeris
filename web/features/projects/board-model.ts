import type { TaskSummary } from "../../api/generated/types";
export const boardColumns = [
  { id: "waiting", title: "待ち", statuses: ["draft", "ready"] },
  { id: "in_progress", title: "進行中", statuses: ["running", "reviewing"] },
  { id: "blocked", title: "停止中", statuses: ["blocked"] },
  { id: "done", title: "完了", statuses: ["done"] },
  { id: "failed", title: "失敗", statuses: ["failed"] },
  { id: "cancelled", title: "中止", statuses: ["cancelled"] },
] as const;
export type BoardFilters = {
  project?: string;
  q?: string;
  label?: string;
  category?: string;
  tier?: string;
  priority?: string;
  assignee?: string;
  milestone?: string;
  show_support?: boolean;
};
export function boardFilterFromSearch(search: URLSearchParams): BoardFilters {
  return {
    project: search.get("project") || undefined,
    q: search.get("q") || undefined,
    label: search.get("label") || undefined,
    category: search.get("category") || undefined,
    tier: search.get("tier") || undefined,
    priority: search.get("priority") || undefined,
    assignee: search.get("assignee") || undefined,
    milestone: search.get("milestone") || undefined,
    show_support: search.get("show_support") === "1",
  };
}
export function boardTasksPath(filters: BoardFilters): string {
  const params = new URLSearchParams();
  for (const key of ["project", "q", "label", "category", "tier", "priority", "assignee", "milestone"] as const) {
    if (filters[key]) params.set(key, filters[key]);
  }
  params.set("limit", "200");
  params.set("order", "created_desc");
  return `/api/tasks?${params}`;
}
export function boardGroup(items: TaskSummary[]) {
  return boardColumns.map((column) => ({
    ...column,
    items: items
      .filter((item) => (column.statuses as readonly string[]).includes(item.status))
      .sort((a, b) => b.priority - a.priority || a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id)),
  }));
}
