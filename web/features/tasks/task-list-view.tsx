import { useQuery } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { TaskList, TaskSummary } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button, buttonClassName } from "../../components/ui/button";
import { StatusBadge, statusView } from "../../components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { mergeTaskPages } from "./merge-pages";
import { type TaskListFilters, taskListPath, taskListQuery } from "./task-list-query";

const STATUSES = ["draft", "ready", "running", "blocked", "reviewing", "done", "failed", "cancelled"] as const;
const ORDERS = [
  ["updated_desc", "更新が新しい順"],
  ["dispatch", "ディスパッチ順"],
  ["created_desc", "作成が新しい順"],
] as const;

const inputClass =
  "min-h-11 min-w-0 rounded-md border border-input bg-surface px-3 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

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
    <div data-screen="/tasks" className="flex min-w-0 flex-col gap-4">
      <h1
        tabIndex={-1}
        className="break-words text-xl font-semibold text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      >
        タスク
      </h1>

      <form
        ref={formRef}
        className="flex min-w-0 flex-wrap items-center gap-3"
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
            className={`${inputClass} w-44 max-w-full`}
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
              className={`${buttonClassName} relative cursor-pointer focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-ring ${status.includes(s) ? "bg-accent font-semibold" : ""}`}
            >
              <input
                type="checkbox"
                name="status"
                value={s}
                aria-label={s}
                defaultChecked={status.includes(s)}
                className="absolute inset-0 h-full min-h-11 w-full min-w-11 cursor-pointer opacity-0"
              />{" "}
              {statusView(s).label}
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
        <Button type="submit">絞り込み</Button>
      </form>

      <FetchFrame query={base}>{merged ? <TaskTable items={merged.items} total={merged.total} /> : null}</FetchFrame>
      {moreError && <p role="alert">続きの取得に失敗しました。もう一度お試しください。</p>}

      {merged && nextCursor !== null ? (
        <div className="flex justify-center">
          <Button type="button" data-testid="tasks-load-more" onClick={() => void loadMore()} disabled={loadingMore}>
            {loadingMore ? "読み込み中…" : "さらに読む"}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function TaskTable({ items, total }: { items: TaskSummary[]; total: number }) {
  return (
    <section aria-label="タスク一覧" className="min-w-0 border-y border-border bg-surface">
      <p className="border-b border-border px-3 py-2 text-label text-muted-foreground" data-testid="tasks-total">
        {total} 件
      </p>
      <div className="max-h-96 min-w-0 overflow-y-auto" data-testid="tasks-scroll">
        <Table className="block w-full sm:table sm:table-fixed" wrapperClassName="overflow-x-hidden">
          <TableHeader className="hidden sm:table-header-group">
            <TableRow>
              <TableHead className="w-1/3">タスク</TableHead>
              <TableHead>状態</TableHead>
              <TableHead>担当</TableHead>
              <TableHead>現在の run</TableHead>
              <TableHead>更新</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody className="block sm:table-row-group">
            {items.map((item) => (
              <TableRow
                key={item.id}
                className="flex min-w-0 flex-wrap border-b border-border px-3 py-1 sm:table-row sm:border-0 sm:px-0 sm:py-0"
                data-task-id={item.id}
              >
                <TableCell className="w-full min-w-0 px-0 py-0 sm:w-auto sm:px-2 sm:py-1">
                  <Link
                    to="/tasks/$id"
                    params={{ id: item.id }}
                    className="flex min-h-11 min-w-0 flex-col justify-center text-foreground underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
                  >
                    <span className="block max-w-full truncate font-medium" title={item.title}>
                      {item.title}
                    </span>
                    <span className="block max-w-full truncate text-label text-muted-foreground" title={item.id}>
                      {item.id}
                    </span>
                  </Link>
                </TableCell>
                <TableCell
                  className="flex w-1/2 min-w-0 items-center gap-1 px-0 py-1 sm:table-cell sm:w-auto sm:px-2"
                  data-status={item.status}
                >
                  <span className="sr-only sm:hidden">状態 </span>
                  <StatusBadge status={item.status} />
                </TableCell>
                <TableCell className="flex w-1/2 min-w-0 items-center gap-1 px-0 py-1 sm:table-cell sm:w-auto sm:px-2">
                  <span className="shrink-0 text-label text-muted-foreground sm:hidden">担当</span>
                  <span className="min-w-0 truncate" title={item.assignee ?? "担当なし"}>
                    {item.assignee ?? "—"}
                  </span>
                </TableCell>
                <TableCell
                  className="flex w-1/2 min-w-0 items-center gap-1 px-0 py-1 text-label text-muted-foreground sm:table-cell sm:w-auto sm:px-2"
                  title="一覧 API に現在の run 情報はありません"
                >
                  <span className="shrink-0 sm:hidden">run</span>
                  <span>情報なし</span>
                </TableCell>
                <TableCell
                  className="flex w-1/2 min-w-0 items-center gap-1 px-0 py-1 text-label text-muted-foreground sm:table-cell sm:w-auto sm:px-2"
                  title={item.updated_at}
                >
                  <span className="shrink-0 sm:hidden">更新</span>
                  <time dateTime={item.updated_at} className="min-w-0 truncate">
                    {new Date(item.updated_at).toLocaleString("ja-JP", { dateStyle: "short", timeStyle: "short" })}
                  </time>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        {items.length === 0 ? (
          <p className="p-6 text-center text-label text-muted-foreground" role="status">
            条件に一致するタスクがありません
          </p>
        ) : null}
      </div>
    </section>
  );
}
