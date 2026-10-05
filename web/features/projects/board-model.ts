import type { TaskSummary } from "../../api/generated/types";
export const boardColumns = [
  { id: "waiting", title: "待ち", statuses: ["draft", "ready"] },
  { id: "in_progress", title: "進行中", statuses: ["running", "reviewing"] },
  { id: "blocked", title: "停止中", statuses: ["blocked"] },
  { id: "done", title: "完了", statuses: ["done"] },
  { id: "failed", title: "失敗", statuses: ["failed"] },
  { id: "cancelled", title: "中止", statuses: ["cancelled"] },
] as const;
export type BoardColumnId = (typeof boardColumns)[number]["id"];
/** daemon へ渡す絞り込み。show_support と column は表示だけの絞り込みで、daemon には渡さない。 */
export const boardQueryFields = [
  "project",
  "q",
  "label",
  "category",
  "tier",
  "priority",
  "assignee",
  "milestone",
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
  column?: BoardColumnId;
};
function columnFromSearch(value: string | null): BoardColumnId | undefined {
  return boardColumns.find((column) => column.id === value)?.id;
}
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
    column: columnFromSearch(search.get("column")),
  };
}
/** 絞り込みを /board の URL に戻す。状態の切り替えは他の絞り込みを保ったまま column だけを替える。 */
export function boardHref(filters: BoardFilters): string {
  const params = new URLSearchParams();
  for (const key of boardQueryFields) {
    const value = filters[key]?.trim();
    if (value) params.set(key, value);
  }
  if (filters.show_support) params.set("show_support", "1");
  if (filters.column) params.set("column", filters.column);
  return `/board${params.size ? `?${params}` : ""}`;
}
export function boardTasksPath(filters: BoardFilters): string {
  const params = new URLSearchParams();
  for (const key of boardQueryFields) {
    const value = filters[key];
    if (value) params.set(key, value);
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

// 内部の英語値を画面の言葉へ。未知の値は「未確認」にして英語を出さない（W-23）。
export const tierLabels: Readonly<Record<string, string>> = {
  cheap: "軽量",
  standard: "標準",
  frontier: "最上位",
};
export const categoryLabels: Readonly<Record<string, string>> = {
  feature: "機能",
  bug: "不具合",
  research: "調査",
  ops: "運用",
  docs: "文書",
  other: "その他",
};
export const priorityLabels: Readonly<Record<string, string>> = {
  urgent: "至急",
  high: "高",
  normal: "通常",
  low: "低",
};
export function labelOf(labels: Readonly<Record<string, string>>, value: string | null | undefined): string {
  if (!value) return "なし";
  return labels[value] ?? (/^P\d$/.test(value) ? value : "未確認");
}
