import { useQuery } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { TaskList, TaskSummary } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { buttonClassName } from "../../components/ui/button";
import { mergeTaskPages } from "./merge-pages";
import { type TaskListFilters, taskListPath, taskListQuery } from "./task-list-query";

const STATUSES = ["draft", "ready", "running", "blocked", "reviewing", "done", "failed", "cancelled"] as const;
const ORDERS = [
  ["updated_desc", "更新が新しい順"],
  ["dispatch", "ディスパッチ順"],
  ["created_desc", "作成が新しい順"],
] as const;

const inputClass = "min-h-11 rounded border border-neutral-400 px-3 text-base";

// 絞り込みの組合せを 1 つの文字列にする。extra（続き）はこの文字列と紐づく。
function filterKey(filters: TaskListFilters): string {
  return JSON.stringify([
    filters.q ?? "",
    [...(filters.status ?? [])].sort(),
    filters.order ?? "",
    filters.limit ?? "",
  ]);
}

// /tasks の画面（P3-05、R21）。絞り込み・並び・検索は search param、続きの読み込みは cursor 付きの取得。
// 画面の実体は features/ に置き、routes/ は配置だけにする（§7 の規則）。
export function TasksListScreen({
  q,
  status,
  order,
  limit,
}: {
  q: string | undefined;
  status: readonly string[];
  order: string | undefined;
  limit: string | undefined;
}) {
  const router = useRouter();
  const formRef = useRef<HTMLFormElement>(null);
  // History navigation changes the URL without remounting the form. Update its
  // controls in place so the focused search input stays focused on submit.
  useEffect(() => {
    const form = formRef.current;
    if (!form) return;
    const search = form.elements.namedItem("q");
    const sort = form.elements.namedItem("order");
    const pageSize = form.elements.namedItem("limit");
    if (search instanceof HTMLInputElement) search.value = q ?? "";
    if (sort instanceof HTMLSelectElement) sort.value = order ?? "updated_desc";
    if (pageSize instanceof HTMLSelectElement) pageSize.value = limit ?? "";
    for (const checkbox of form.querySelectorAll<HTMLInputElement>('input[name="status"]')) {
      checkbox.checked = status.includes(checkbox.value);
    }
  }, [q, status, order, limit]);
  const filters: TaskListFilters = { q, status, order, limit };
  const key = filterKey(filters);
  const base = useQuery({ ...taskListQuery(filters) });

  // 続き（cursor で取得したページ群）は local state。絞り込みが変わると key がずれて無視されるので、
  // SSE の取り直しで最初のページが変わっても読み込んだ分は id 重複を除いて消えない。
  const [more, setMore] = useState<{ key: string; pages: TaskList[]; cursor: string | null }>({
    key,
    pages: [],
    cursor: null,
  });
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState(false);
  const effective = more.key === key ? more : { key, pages: [] as TaskList[], cursor: null };
  const merged = base.data ? mergeTaskPages(base.data, effective.pages) : null;
  const nextCursor = merged ? (effective.pages.length > 0 ? effective.cursor : (base.data?.next_cursor ?? null)) : null;

  async function loadMore() {
    const cursor = nextCursor;
    if (cursor === null || loadingMore) return;
    setLoadingMore(true);
    setMoreError(false);
    try {
      const page = await apiGet<TaskList>(taskListPath(filters, cursor));
      setMore({ key, pages: [...effective.pages, page], cursor: page.next_cursor ?? null });
    } catch {
      setMoreError(true);
    } finally {
      setLoadingMore(false);
    }
  }

  function applyFilters(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const nextQ = String(data.get("q") ?? "").trim() || undefined;
    const nextStatus = (data.getAll("status") as string[]).filter((s) => (STATUSES as readonly string[]).includes(s));
    const nextOrder = String(data.get("order") ?? "updated_desc");
    const nextLimit = String(data.get("limit") ?? "").trim() || undefined;
    const params = new URLSearchParams();
    if (nextQ) params.set("q", nextQ);
    for (const value of nextStatus) params.append("status", value);
    params.set("order", nextOrder);
    if (nextLimit) params.set("limit", nextLimit);
    router.history.push(`/tasks?${params}`, {});
  }

  return (
    <div data-screen="/tasks" className="flex flex-col gap-4">
      <h1 tabIndex={-1} className="text-xl font-semibold break-words focus:outline-none">
        タスク
      </h1>

      <form
        ref={formRef}
        className="flex flex-wrap items-center gap-3"
        onSubmit={applyFilters}
        data-testid="tasks-filter"
      >
        <label className="flex items-center gap-2 text-sm">
          <span id="tasks-q">検索</span>
          <input
            type="search"
            name="q"
            aria-labelledby="tasks-q"
            defaultValue={q ?? ""}
            placeholder="タイトル・id"
            className={`${inputClass} w-44`}
          />
        </label>
        <label className="flex items-center gap-2 text-sm">
          <span>件数</span>
          <select name="limit" defaultValue={limit ?? ""} className={inputClass}>
            <option value="">既定</option>
            <option value="20">20</option>
            <option value="50">50</option>
            <option value="100">100</option>
          </select>
        </label>
        <fieldset className="flex flex-wrap items-center gap-1.5">
          <legend className="sr-only">status の絞り込み</legend>
          {STATUSES.map((s) => (
            <label
              key={s}
              className={`${buttonClassName} relative min-h-12 cursor-pointer focus-within:ring-2 focus-within:ring-blue-600 ${status.includes(s) ? "bg-neutral-200 font-semibold" : ""}`}
            >
              <input
                type="checkbox"
                name="status"
                value={s}
                defaultChecked={status.includes(s)}
                className="absolute inset-0 h-full w-full cursor-pointer opacity-0"
              />{" "}
              {s}
            </label>
          ))}
        </fieldset>
        <label className="flex items-center gap-2 text-sm">
          <span id="tasks-order">並び</span>
          <select
            name="order"
            aria-labelledby="tasks-order"
            defaultValue={order ?? "updated_desc"}
            className={inputClass}
          >
            {ORDERS.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" className={buttonClassName}>
          絞り込み
        </button>
      </form>

      <FetchFrame query={base}>{merged ? <TaskTable items={merged.items} total={merged.total} /> : null}</FetchFrame>
      {moreError && <p role="alert">続きの取得に失敗しました。もう一度お試しください。</p>}

      {merged && nextCursor !== null ? (
        <div className="flex justify-center">
          <button
            type="button"
            className={buttonClassName}
            data-testid="tasks-load-more"
            onClick={() => void loadMore()}
            disabled={loadingMore}
          >
            {loadingMore ? "読み込み中…" : "さらに読む"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

function TaskTable({ items, total }: { items: TaskSummary[]; total: number }) {
  return (
    <div className="rounded border border-neutral-300">
      <p className="border-b border-neutral-200 px-3 py-2 text-sm" data-testid="tasks-total">
        {total} 件
      </p>
      <div className="max-h-96 overflow-auto" data-testid="tasks-scroll">
        <table className="w-full min-w-112 border-collapse text-sm">
          <thead className="sticky top-0 bg-neutral-50 text-left">
            <tr>
              <th scope="col" className="px-3 py-2">
                タイトル
              </th>
              <th scope="col" className="px-3 py-2">
                状態
              </th>
              <th scope="col" className="px-3 py-2">
                種別
              </th>
              <th scope="col" className="px-3 py-2">
                担当
              </th>
              <th scope="col" className="px-3 py-2">
                更新
              </th>
            </tr>
          </thead>
          <tbody>
            {items.map((item) => (
              <tr key={item.id} className="border-t border-neutral-200" data-task-id={item.id}>
                <td className="max-w-56 px-3 py-2">
                  <Link to="/tasks/$id" params={{ id: item.id }} className="break-words underline underline-offset-2">
                    {item.title}
                  </Link>
                </td>
                <td className="px-3 py-2" data-status={item.status}>
                  {item.status}
                </td>
                <td className="px-3 py-2">{item.kind}</td>
                <td className="px-3 py-2">{item.assignee ?? "-"}</td>
                <td className="px-3 py-2 text-xs text-neutral-500" title={item.updated_at}>
                  {item.updated_at}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {items.length === 0 ? (
          <p className="p-6 text-center text-sm text-neutral-500" role="status">
            条件に一致するタスクがありません
          </p>
        ) : null}
      </div>
    </div>
  );
}
