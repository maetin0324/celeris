import type { TaskList, TaskSummary } from "../../api/generated/types";

// 続きの読み込み（P3-05）。最初のページ（query の data）に追加ページを id 重複なしで繋げる。
// 純関数で、SSE の取り直し（同じ最初のページ）で読み込んだ分が消えないことを確かめやすくする。
export type MergedPages = { items: TaskSummary[]; nextCursor: string | null; total: number };

export function mergeTaskPages(first: TaskList, extra: readonly TaskList[]): MergedPages {
  const seen = new Set<string>();
  const items: TaskSummary[] = [];
  const push = (item: TaskSummary) => {
    if (seen.has(item.id)) return;
    seen.add(item.id);
    items.push(item);
  };
  first.items.forEach(push);
  for (const page of extra) page.items.forEach(push);
  const last = extra.length > 0 ? extra[extra.length - 1] : first;
  const cursor = last.next_cursor;
  return { items, nextCursor: cursor === undefined || cursor === "" ? null : cursor, total: first.total };
}
