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
  run_state?: "runnable" | "waiting";
};
function columnFromSearch(value: string | null): BoardColumnId | undefined {
  return boardColumns.find((column) => column.id === value)?.id;
}
export function boardFilterFromSearch(search: URLSearchParams): BoardFilters {
  const runState = search.get("run_state");
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
    run_state: runState === "runnable" || runState === "waiting" ? runState : undefined,
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
  if (filters.run_state) params.set("run_state", filters.run_state);
  return `/board${params.size ? `?${params}` : ""}`;
}
/** 終端でない状態。絞り込みに合えば作成日時や件数に関係なく全件をボードに出す。 */
export const liveStatuses = ["draft", "ready", "running", "reviewing", "blocked"] as const;
/** 終端の状態。列ごとに作成の新しい順で `terminalLimit` 件だけ読む。 */
export const terminalStatuses = ["done", "failed", "cancelled"] as const;
export type TerminalStatus = (typeof terminalStatuses)[number];
export const terminalLimit = 200;
/** daemon の `limit` の上限。終端でない task はこの大きさの頁で `next_cursor` を辿って全件読む。 */
const livePageLimit = 500;

function baseParams(filters: BoardFilters): URLSearchParams {
  const params = new URLSearchParams();
  for (const key of boardQueryFields) {
    const value = filters[key];
    if (value) params.set(key, value);
  }
  return params;
}

/** 終端でない task の 1 頁。`cursor` は前の頁の `next_cursor`。 */
export function boardLiveTasksPath(filters: BoardFilters, cursor?: string): string {
  const params = baseParams(filters);
  params.set("status", liveStatuses.join(","));
  params.set("limit", String(livePageLimit));
  params.set("order", "created_desc");
  if (filters.run_state) params.set("run_state", filters.run_state);
  if (cursor) params.set("cursor", cursor);
  return `/api/tasks?${params}`;
}

/** 終端の 1 状態の新しい `terminalLimit` 件。`total` がその状態の実際の件数になる。 */
export function boardTerminalTasksPath(filters: BoardFilters, status: TerminalStatus): string {
  const params = baseParams(filters);
  params.set("status", status);
  params.set("limit", String(terminalLimit));
  params.set("order", "created_desc");
  return `/api/tasks?${params}`;
}

/** 終端の列の読み込み範囲。`total` は daemon が数えた件数（裏方を含む）、`loaded` は読んだ件数。 */
export type TerminalRange = { loaded: number; total: number };
export type BoardData = {
  items: TaskSummary[];
  terminal: Partial<Record<TerminalStatus, TerminalRange>>;
};

type TaskPage = { items: TaskSummary[]; next_cursor?: string | null; total: number };

/**
 * ボードの task を読む。終端でない task は全頁を辿って全件、終端の task は状態ごとに新しい順の
 * `terminalLimit` 件だけ読む。`run_state` は終端でない task だけの絞り込みなので、そのときは終端を読まない。
 */
export async function fetchBoard(filters: BoardFilters, get: (path: string) => Promise<TaskPage>): Promise<BoardData> {
  const liveTask = (async () => {
    const items: TaskSummary[] = [];
    let cursor: string | undefined;
    const seen = new Set<string>();
    for (;;) {
      const page = await get(boardLiveTasksPath(filters, cursor));
      items.push(...page.items);
      const next = page.next_cursor ?? undefined;
      // 同じ cursor が返ったら打ち切る（無限に読まない）。
      if (!next || seen.has(next)) return items;
      seen.add(next);
      cursor = next;
    }
  })();
  const statuses = filters.run_state ? [] : terminalStatuses;
  const terminalPages = Promise.all(
    statuses.map(async (status) => [status, await get(boardTerminalTasksPath(filters, status))] as const),
  );
  const [live, pages] = await Promise.all([liveTask, terminalPages]);
  const terminal: BoardData["terminal"] = {};
  const items = [...live];
  for (const [status, page] of pages) {
    items.push(...page.items);
    terminal[status] = { loaded: page.items.length, total: Math.max(page.total, page.items.length) };
  }
  return { items, terminal };
}

export type BoardGroup = (typeof boardColumns)[number] & {
  items: TaskSummary[];
  /** 列の件数。読み切った列は表示する件数、件数制限で切った列は daemon が数えた件数。 */
  count: number;
  /** 件数制限で一部しか読んでいない列の範囲。読み切った列は `undefined`。 */
  truncated?: TerminalRange;
};

export function boardGroup(items: TaskSummary[], terminal: BoardData["terminal"] = {}): BoardGroup[] {
  return boardColumns.map((column) => {
    const columnItems = items
      .filter((item) => (column.statuses as readonly string[]).includes(item.status))
      .sort(
        (a, b) =>
          Number(b.runnable) - Number(a.runnable) ||
          b.priority - a.priority ||
          a.created_at.localeCompare(b.created_at) ||
          a.id.localeCompare(b.id),
      );
    const range = column.statuses.length === 1 ? terminal[column.statuses[0] as TerminalStatus] : undefined;
    const truncated = range && range.total > range.loaded ? range : undefined;
    return { ...column, items: columnItems, count: truncated ? truncated.total : columnItems.length, truncated };
  });
}

/** 列見出しの件数。件数制限で切った列は全件の数と表示している件数を並べる。 */
export function boardGroupCountLabel(group: BoardGroup): string {
  return group.truncated
    ? `全 ${group.truncated.total} 件のうち新しい ${group.truncated.loaded} 件を表示`
    : `${group.items.length} 件`;
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
