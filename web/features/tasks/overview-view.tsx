import { Link } from "@tanstack/react-router";
import type { TaskDetail } from "../../api/generated/types";

// /tasks/:id の概要（P3-08、表示のみ）。操作（判断・実行・routing）は P3-09 / P3-10 で足す。
// 枠の中で折り返し・スクロールして、ページ全体の横溢れを出さない（S3）。

const dl = "min-w-0 grid grid-cols-[minmax(6rem,10rem)_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm";
const dt = "text-neutral-500";

export function OverviewView({ detail }: { detail: TaskDetail }) {
  const task = detail.task;
  return (
    <div className="flex flex-col gap-4" data-testid="task-overview">
      <section className="min-w-0 rounded border border-neutral-300 p-3">
        <h2 className="text-base font-semibold break-words">{task.title}</h2>
        <dl className={`${dl} mt-2`}>
          <dt className={dt}>状態</dt>
          <dd data-status={task.status}>{task.status}</dd>
          <dt className={dt}>種別</dt>
          <dd>{task.kind}</dd>
          {task.category ? (
            <>
              <dt className={dt}>カテゴリ</dt>
              <dd>{task.category}</dd>
            </>
          ) : null}
          {detail.priority_label ? (
            <>
              <dt className={dt}>優先度</dt>
              <dd>{detail.priority_label}</dd>
            </>
          ) : null}
          {task.assignee ? (
            <>
              <dt className={dt}>担当</dt>
              <dd>{task.assignee}</dd>
            </>
          ) : null}
          {task.genre ? (
            <>
              <dt className={dt}>分野</dt>
              <dd>{task.genre}</dd>
            </>
          ) : null}
          <dt className={dt}>試行</dt>
          <dd>
            {task.attempts} / {task.budget.max_retries}
          </dd>
          <dt className={dt}>作成</dt>
          <dd>{task.created_at}</dd>
          <dt className={dt}>更新</dt>
          <dd>{task.updated_at}</dd>
        </dl>
      </section>

      {task.objective ? (
        <section className="min-w-0 rounded border border-neutral-300 p-3">
          <h2 className="text-base font-semibold">目的</h2>
          <p className="mt-1 text-sm whitespace-pre-wrap break-words">{task.objective}</p>
        </section>
      ) : null}

      {detail.failure ? (
        <section className="min-w-0 rounded border border-red-300 bg-red-50 p-3" role="alert">
          <h2 className="text-base font-semibold">失敗</h2>
          <p className="mt-1 text-sm break-words">
            {detail.failure.class}: {detail.failure.reason}
          </p>
        </section>
      ) : null}

      {detail.latest_question ? (
        <section className="min-w-0 rounded border border-amber-300 bg-amber-50 p-3">
          <h2 className="text-base font-semibold">質問待ち</h2>
          <p className="mt-1 text-sm whitespace-pre-wrap break-words">{detail.latest_question}</p>
        </section>
      ) : null}

      <RelatedTasks title="依存" items={detail.dependencies} />
      <RelatedTasks title="依存元" items={detail.dependents} />
      <RelatedTasks title="子タスク" items={detail.children} />

      {detail.criteria.length > 0 ? (
        <section className="min-w-0 rounded border border-neutral-300 p-3">
          <h2 className="text-base font-semibold">受け入れ条件（{detail.criteria.length}）</h2>
          <ul className="mt-2 flex flex-col gap-2">
            {detail.criteria.map((criterion) => (
              <li key={criterion.idx} className="min-w-0 text-sm">
                <span className="font-medium">{criterion.idx}: </span>
                <span className="break-words">{criterion.text}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {detail.runs.length > 0 ? (
        <section className="min-w-0 rounded border border-neutral-300 p-3">
          <h2 className="text-base font-semibold">run（{detail.runs.length}）</h2>
          <ul className="mt-2 flex flex-col gap-1">
            {detail.runs.map((run) => (
              <li key={run.run_id} className="text-sm">
                <Link
                  to="/tasks/$id/runs/$runId"
                  params={{ id: task.id, runId: run.run_id }}
                  className="break-words underline underline-offset-2"
                >
                  {run.run_id}
                </Link>{" "}
                <span className="text-neutral-500">
                  {run.adapter} / {run.model}
                </span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function RelatedTasks({ title, items }: { title: string; items: TaskDetail["dependencies"] }) {
  if (items.length === 0) return null;
  return (
    <section className="min-w-0 rounded border border-neutral-300 p-3">
      <h2 className="text-base font-semibold">
        {title}（{items.length}）
      </h2>
      <ul className="mt-2 flex flex-wrap gap-1">
        {items.map((item) => (
          <li key={item.id} className="text-sm">
            <Link to="/tasks/$id" params={{ id: item.id }} className="break-words underline underline-offset-2">
              {item.title}
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
