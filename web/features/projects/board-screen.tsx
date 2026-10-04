import { useQuery } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { TaskList, TaskSummary } from "../../api/generated/types";
import { boardKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { StatusBadge } from "../../components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { cn } from "../../lib/utils";
import {
  type BoardColumnId,
  type BoardFilters,
  boardColumns,
  boardFilterFromSearch,
  boardGroup,
  boardHref,
  boardQueryFields,
  boardTasksPath,
} from "./board-model";
import { projectListQuery } from "./project-queries";

// 入力欄の枠は --color-input（3:1）。入力とボタンは同じ最小高さ（DESIGN.md「余白」）。
const control =
  "block min-h-11 w-full min-w-0 rounded-md border border-input bg-surface px-2 text-body text-foreground";
const fieldLabel = "flex min-w-0 flex-col gap-1 text-label font-medium";
// 狭い幅では 優先度・レベル・担当・種類 の列を畳み、題名の下の 1 行で読む。
const wideCell = "hidden md:table-cell";
const columnCount = 7;

function searchObject(filters: BoardFilters): Record<string, string> {
  return Object.fromEntries(new URLSearchParams(boardHref(filters).split("?")[1] ?? ""));
}

function BoardEditRow({
  task,
  filterKey,
  formId,
  onClose,
}: {
  task: TaskSummary;
  filterKey: ReturnType<typeof boardKeys.list>;
  formId: string;
  onClose: () => void;
}) {
  const sender = useActionResult(filterKey);
  const [priority, setPriority] = useState(task.priority_label);
  const [tier, setTier] = useState<string>(task.tier);
  const [assignee, setAssignee] = useState(task.assignee ?? "");
  const result = sender.results[task.id];
  async function edit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const body: Record<string, unknown> = { expected_status: task.status };
    if (priority !== task.priority_label) body.priority = priority;
    if (tier !== task.tier) body.tier = tier;
    if (assignee !== (task.assignee ?? "")) body.assignee = assignee || null;
    if (Object.keys(body).length === 1) return;
    await sender.run([{ id: task.id, method: "PATCH", path: `/api/tasks/${encodeURIComponent(task.id)}`, body }]);
  }
  return (
    <TableRow className="bg-muted hover:bg-muted" data-task-edit={task.id}>
      <TableCell colSpan={columnCount} className="px-3 py-3">
        <form
          id={formId}
          onSubmit={edit}
          aria-label={`${task.title} を編集`}
          className="grid max-w-form grid-cols-1 items-end gap-3 sm:grid-cols-3"
        >
          <label className={fieldLabel}>
            優先度
            <select className={control} value={priority} onChange={(e) => setPriority(e.target.value)}>
              {["P0", "P1", "P2", "P3"].map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
          <label className={fieldLabel}>
            レベル
            <select className={control} value={tier} onChange={(e) => setTier(e.target.value)}>
              {["cheap", "standard", "frontier"].map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
          <label className={fieldLabel}>
            担当
            <input className={control} value={assignee} onChange={(e) => setAssignee(e.target.value)} />
          </label>
          <div className="flex flex-wrap gap-2 sm:col-span-3">
            <Button type="submit" variant="primary" disabled={sender.pending}>
              保存
            </Button>
            <Button type="button" variant="secondary" onClick={onClose}>
              閉じる
            </Button>
          </div>
        </form>
        <ActionResultView result={result} />
      </TableCell>
    </TableRow>
  );
}

function BoardRow({ task, filterKey }: { task: TaskSummary; filterKey: ReturnType<typeof boardKeys.list> }) {
  const [open, setOpen] = useState(false);
  const formId = useId();
  const editable = task.actions.includes("edit");
  return (
    <>
      <TableRow data-task-id={task.id}>
        <TableCell className="min-w-0">
          <Link
            className="inline-flex min-h-11 items-center break-words font-medium text-primary underline"
            to="/tasks/$id"
            params={{ id: task.id }}
          >
            {task.title}
          </Link>
          <p className="break-words text-muted-foreground md:hidden">
            {task.priority_label} · {task.tier} · 担当 {task.assignee || "なし"}
          </p>
        </TableCell>
        <TableCell>
          <StatusBadge status={task.status} />
        </TableCell>
        <TableCell className={cn(wideCell, "tabular-nums")}>{task.priority_label}</TableCell>
        <TableCell className={wideCell}>{task.tier}</TableCell>
        <TableCell className={cn(wideCell, "break-words")}>{task.assignee || "なし"}</TableCell>
        <TableCell className={cn(wideCell, "break-words")}>{task.category}</TableCell>
        <TableCell className="text-right">
          {editable && (
            <Button
              type="button"
              variant="secondary"
              aria-expanded={open}
              aria-controls={open ? formId : undefined}
              aria-label={`${task.title} を編集`}
              onClick={() => setOpen((value) => !value)}
            >
              編集
            </Button>
          )}
        </TableCell>
      </TableRow>
      {editable && open && (
        <BoardEditRow task={task} filterKey={filterKey} formId={formId} onClose={() => setOpen(false)} />
      )}
    </>
  );
}

function StateFilter({
  filter,
  counts,
  total,
}: {
  filter: BoardFilters;
  counts: Record<BoardColumnId, number>;
  total: number;
}) {
  const options: { id: BoardColumnId | undefined; title: string; count: number }[] = [
    { id: undefined, title: "すべて", count: total },
    ...boardColumns.map((column) => ({ id: column.id, title: column.title, count: counts[column.id] })),
  ];
  return (
    <nav aria-label="状態で絞り込む" className="min-w-0">
      <ul className="flex flex-wrap gap-2">
        {options.map((option) => {
          const current = filter.column === option.id;
          return (
            <li key={option.id ?? "all"}>
              <Link
                to="/board"
                search={searchObject({ ...filter, column: option.id })}
                aria-current={current ? "page" : undefined}
                className={cn(
                  "inline-flex min-h-11 items-center gap-1 rounded-md border px-3 text-label",
                  current
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-input bg-surface text-foreground hover:bg-accent",
                )}
              >
                {option.title}
                <span className="tabular-nums">{option.count}</span>
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

const extraFields = [
  { name: "label", title: "ラベル" },
  { name: "category", title: "種類" },
  { name: "tier", title: "レベル" },
  { name: "assignee", title: "担当" },
  { name: "milestone", title: "途中目標" },
] as const;

export function BoardScreen({ searchStr }: { searchStr: string }) {
  const router = useRouter();
  const form = useRef<HTMLFormElement>(null);
  const filter = boardFilterFromSearch(new URLSearchParams(searchStr));
  const key = boardKeys.list(filter);
  const projects = useQuery(projectListQuery({}));
  const tasks = useQuery({ queryKey: key, queryFn: ({ signal }) => apiGet<TaskList>(boardTasksPath(filter), signal) });
  // Browser history is the source of truth. Keep the controls in sync after Back/Forward.
  useEffect(() => {
    if (!form.current || !projects.data?.items) return;
    const current = boardFilterFromSearch(new URLSearchParams(searchStr));
    for (const name of boardQueryFields) {
      const field = form.current.elements.namedItem(name);
      if (field instanceof HTMLInputElement || field instanceof HTMLSelectElement) field.value = current[name] ?? "";
    }
    const support = form.current.elements.namedItem("show_support");
    if (support instanceof HTMLInputElement) support.checked = !!current.show_support;
  }, [searchStr, projects.data?.items]);
  function apply(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const fields = new FormData(event.currentTarget);
    const next: BoardFilters = { show_support: fields.has("show_support"), column: filter.column };
    for (const name of boardQueryFields) next[name] = String(fields.get(name) ?? "").trim() || undefined;
    router.history.push(boardHref(next), {});
  }
  const visible = (filter.show_support ? tasks.data?.items : tasks.data?.items.filter((task) => !task.support)) ?? [];
  const groups = boardGroup(visible);
  const counts = Object.fromEntries(groups.map((group) => [group.id, group.items.length])) as Record<
    BoardColumnId,
    number
  >;
  const shown = groups.filter((group) => (filter.column ? group.id === filter.column : group.items.length > 0));
  const extraOpen = extraFields.some((field) => filter[field.name]) || !!filter.priority || !!filter.show_support;
  return (
    <ScreenFrame title="ボード" route="/board">
      <p className="max-w-prose text-muted-foreground">
        案件のタスクを状態ごとにまとめた一覧です。優先度・レベル・担当は行の「編集」から変えます。
      </p>
      <form ref={form} onSubmit={apply} className="min-w-0 space-y-3" data-testid="board-filter-form">
        <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-end">
          <label className={cn(fieldLabel, "sm:flex-1")}>
            案件
            <select name="project" className={control} defaultValue={filter.project ?? ""}>
              <option value="">すべての案件</option>
              {projects.data?.items.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.title}
                </option>
              ))}
            </select>
          </label>
          <label className={cn(fieldLabel, "sm:flex-1")}>
            検索
            <input name="q" type="search" className={control} defaultValue={filter.q} />
          </label>
          <Button type="submit" variant="primary">
            絞り込む
          </Button>
        </div>
        <details open={extraOpen} className="min-w-0">
          <summary className="inline-flex min-h-11 cursor-pointer items-center text-label font-medium text-primary">
            条件を足す（ラベル・種類・レベル・優先度・担当・途中目標）
          </summary>
          <div className="mt-2 grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {extraFields.map((field) => (
              <label key={field.name} className={fieldLabel}>
                {field.title}
                <input name={field.name} className={control} defaultValue={filter[field.name]} />
              </label>
            ))}
            <label className={fieldLabel}>
              優先度
              <select name="priority" className={control} defaultValue={filter.priority ?? ""}>
                <option value="">すべて</option>
                {["P0", "P1", "P2", "P3"].map((v) => (
                  <option key={v}>{v}</option>
                ))}
              </select>
            </label>
            <label className="flex min-h-11 items-center gap-2 self-end text-label font-medium">
              <input
                name="show_support"
                type="checkbox"
                className="size-5 accent-primary"
                defaultChecked={filter.show_support}
              />
              裏方も表示
            </label>
          </div>
        </details>
      </form>
      <FetchFrame query={projects}>
        {projects.data ? (
          <FetchFrame query={tasks}>
            {tasks.data && (
              <div className="min-w-0 space-y-3">
                <StateFilter filter={filter} counts={counts} total={visible.length} />
                {shown.length === 0 || shown.every((group) => group.items.length === 0) ? (
                  <p className="text-muted-foreground" data-testid="board-empty">
                    {filter.column ? "この状態のタスクはありません。" : "条件に合うタスクはありません。"}
                  </p>
                ) : (
                  <Table
                    aria-label="状態ごとのタスク"
                    wrapperClassName="rounded-lg border border-border bg-surface"
                    data-testid="board-table"
                  >
                    <TableHeader>
                      <TableRow className="hover:bg-transparent">
                        <TableHead className="w-full">タスク</TableHead>
                        <TableHead>状態</TableHead>
                        <TableHead className={wideCell}>優先度</TableHead>
                        <TableHead className={wideCell}>レベル</TableHead>
                        <TableHead className={wideCell}>担当</TableHead>
                        <TableHead className={wideCell}>種類</TableHead>
                        <TableHead>
                          <span className="sr-only">操作</span>
                        </TableHead>
                      </TableRow>
                    </TableHeader>
                    {shown.map((group) => (
                      <TableBody key={group.id} data-board-column={group.id}>
                        <TableRow className="bg-muted hover:bg-muted">
                          <TableHead scope="colgroup" colSpan={columnCount} className="px-3 text-foreground">
                            <h2 className="text-label font-semibold">
                              {group.title}{" "}
                              <span className="tabular-nums text-muted-foreground">{group.items.length} 件</span>
                            </h2>
                          </TableHead>
                        </TableRow>
                        {group.items.map((task) => (
                          <BoardRow key={task.id} task={task} filterKey={key} />
                        ))}
                      </TableBody>
                    ))}
                  </Table>
                )}
              </div>
            )}
          </FetchFrame>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
