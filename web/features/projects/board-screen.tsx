import { useQuery } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { type FormEvent, useEffect, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { TaskList, TaskSummary } from "../../api/generated/types";
import { boardKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { boardFilterFromSearch, boardGroup, boardTasksPath } from "./board-model";
import { projectListQuery } from "./project-queries";

const control = "min-h-11 max-w-full rounded border border-neutral-400 bg-white p-2";
function BoardCard({ task, filterKey }: { task: TaskSummary; filterKey: ReturnType<typeof boardKeys.list> }) {
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
    <article className="min-w-0 space-y-2 rounded border bg-white p-3" data-task-id={task.id}>
      <Link className="block min-h-11 break-words font-medium underline" to="/tasks/$id" params={{ id: task.id }}>
        {task.title}
      </Link>
      <p className="text-sm">
        {task.status} · {task.priority_label}
      </p>
      <p className="break-words text-sm">
        担当: {task.assignee || "（なし）"} · レベル: {task.tier} · 種類: {task.category}
      </p>
      {task.actions.includes("edit") && (
        <form onSubmit={edit} className="flex flex-wrap items-end gap-2">
          <label className="text-sm">
            優先度
            <select className={`${control} block`} value={priority} onChange={(e) => setPriority(e.target.value)}>
              {["P0", "P1", "P2", "P3"].map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
          <label className="text-sm">
            レベル
            <select className={`${control} block`} value={tier} onChange={(e) => setTier(e.target.value)}>
              {["cheap", "standard", "frontier"].map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
          <label className="text-sm">
            担当
            <input className={`${control} block w-32`} value={assignee} onChange={(e) => setAssignee(e.target.value)} />
          </label>
          <Button type="submit" disabled={sender.pending}>
            編集
          </Button>
        </form>
      )}
      <ActionResultView result={result} />
    </article>
  );
}

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
    for (const name of ["project", "q", "label", "category", "tier", "priority", "assignee", "milestone"] as const) {
      const field = form.current.elements.namedItem(name);
      if (field instanceof HTMLInputElement || field instanceof HTMLSelectElement) field.value = current[name] ?? "";
    }
    const support = form.current.elements.namedItem("show_support");
    if (support instanceof HTMLInputElement) support.checked = !!current.show_support;
  }, [searchStr, projects.data?.items]);
  function apply(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const fields = new FormData(event.currentTarget);
    const params = new URLSearchParams();
    for (const name of ["project", "q", "label", "category", "tier", "priority", "assignee", "milestone"]) {
      const value = String(fields.get(name) ?? "").trim();
      if (value) params.set(name, value);
    }
    if (fields.has("show_support")) params.set("show_support", "1");
    router.history.push(`/board${params.size ? `?${params}` : ""}`, {});
  }
  const visible = filter.show_support ? tasks.data?.items : tasks.data?.items.filter((task) => !task.support);
  return (
    <ScreenFrame title="ボード" route="/board">
      <p>案件のタスクを 6 列で表示します。優先度はカードから編集できます。</p>
      <form
        ref={form}
        onSubmit={apply}
        className="flex min-w-0 flex-wrap items-end gap-2 rounded border p-3"
        data-testid="board-filter-form"
      >
        <label>
          案件
          <select name="project" className={`${control} block`} defaultValue={filter.project ?? ""}>
            <option value="">すべての案件</option>
            {projects.data?.items.map((project) => (
              <option key={project.id} value={project.id}>
                {project.title}
              </option>
            ))}
          </select>
        </label>
        <label>
          検索
          <input name="q" className={`${control} block`} defaultValue={filter.q} />
        </label>
        <label>
          ラベル
          <input name="label" className={`${control} block`} defaultValue={filter.label} />
        </label>
        <label>
          種類
          <input name="category" className={`${control} block`} defaultValue={filter.category} />
        </label>
        <label>
          レベル
          <input name="tier" className={`${control} block`} defaultValue={filter.tier} />
        </label>
        <label>
          優先度
          <select name="priority" className={`${control} block`} defaultValue={filter.priority ?? ""}>
            <option value="">すべて</option>
            {["P0", "P1", "P2", "P3"].map((v) => (
              <option key={v}>{v}</option>
            ))}
          </select>
        </label>
        <label>
          担当
          <input name="assignee" className={`${control} block`} defaultValue={filter.assignee} />
        </label>
        <label>
          途中目標
          <input name="milestone" className={`${control} block`} defaultValue={filter.milestone} />
        </label>
        <label className="flex min-h-11 items-center gap-2">
          <input name="show_support" type="checkbox" className="size-11" defaultChecked={filter.show_support} />
          裏方も表示
        </label>
        <Button type="submit">絞り込む</Button>
      </form>
      <FetchFrame query={projects}>
        {projects.data ? (
          <FetchFrame query={tasks}>
            {tasks.data && (
              <div className="min-w-0 max-w-full overflow-x-auto rounded border" data-testid="board-scroll-frame">
                <div className="flex w-max gap-3 p-3">
                  {boardGroup(visible ?? []).map((column) => (
                    <section key={column.id} className="w-64 shrink-0 space-y-2" data-board-column={column.id}>
                      <h2 className="font-semibold">
                        {column.title} ({column.items.length})
                      </h2>
                      {column.items.map((task) => (
                        <BoardCard key={task.id} task={task} filterKey={key} />
                      ))}
                    </section>
                  ))}
                </div>
              </div>
            )}
          </FetchFrame>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
