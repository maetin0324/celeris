import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { taskDetailQuery } from "../tasks/task-detail-query";
import { parseRunLog, type RunLogEvent } from "./run-log";
import { useRunLog } from "./use-run-log";

// /tasks/:id/runs/:runId（P3-12、R26）。見出しは取得を待たずに出す（S1）。
// 実行中かどうかは task 詳細の runs の finished_at で決める（分からない間は追い掛ける。取得に失敗したら終わった扱い）。
export function RunLogScreen({ taskId, runId }: { taskId: string; runId: string }) {
  const detail = useQuery(taskDetailQuery(taskId));
  const run = detail.data?.runs.find((item) => item.run_id === runId);
  const running = detail.isError ? false : detail.data ? Boolean(run && !run.finished_at) : undefined;
  const log = useRunLog(taskId, runId, running);
  const events = useMemo(() => parseRunLog(log.buffer.lines), [log.buffer.lines]);
  return (
    <ScreenFrame title={`run ログ ${taskId} / ${runId}`} route="/tasks/:id/runs/:runId">
      {log.status === "loading" ? (
        <div aria-busy="true" data-fetch-state="loading" className="h-16 animate-pulse rounded bg-neutral-100" />
      ) : log.status === "error" ? (
        <p role="alert" data-fetch-state="error" className="text-sm">
          run ログを取得できませんでした。
        </p>
      ) : (
        <section aria-label="run ログ" className="min-w-0 space-y-2" data-testid="run-log">
          <p className="text-sm text-neutral-700">
            <span data-testid="run-log-line-count" data-count={log.buffer.lines.length}>
              {log.buffer.lines.length} 行
            </span>
            {log.buffer.dropped > 0 ? `（古い ${log.buffer.dropped} 行は省略）` : null}
            {log.following ? "・実行中（追記を追っています）" : null}
          </p>
          <ol className="min-w-0 space-y-2">
            {events.map((event, i) => (
              // 追記は末尾に足すだけなので index で安定する。
              // biome-ignore lint/suspicious/noArrayIndexKey: append-only list
              <li key={i} data-kind={event.kind} className="min-w-0 rounded border border-neutral-200 p-2 text-sm">
                <EventBody event={event} />
              </li>
            ))}
          </ol>
        </section>
      )}
    </ScreenFrame>
  );
}

function EventBody({ event }: { event: RunLogEvent }) {
  const pre = "mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words text-xs";
  switch (event.kind) {
    case "message":
      return (
        <>
          <p className="font-semibold">{event.role === "assistant" ? "agent" : "user"}</p>
          <p className="whitespace-pre-wrap break-words">{event.text}</p>
        </>
      );
    case "thinking":
      return (
        <details>
          <summary>思考</summary>
          <p className={pre}>{event.text}</p>
        </details>
      );
    case "tool":
      return (
        <details>
          <summary className="break-words">
            tool {event.name}
            {event.summary ? `: ${event.summary}` : ""}
            {event.isError ? "（失敗）" : ""}
          </summary>
          {event.result ? <pre className={pre}>{event.result}</pre> : null}
        </details>
      );
    case "command":
      return (
        <details>
          <summary className="break-words">$ {event.command}</summary>
          {event.output ? <pre className={pre}>{event.output}</pre> : null}
        </details>
      );
    case "file_change":
      return <p className="break-words">変更: {event.diffs.map((d) => d.path).join(", ")}</p>;
    case "error":
      return <p className="break-words text-red-800">エラー: {event.text}</p>;
    case "usage":
      return (
        <p className="break-words">
          {event.title}
          {event.stats.length ? ` — ${event.stats.map((s) => `${s.label} ${s.value}`).join("、")}` : ""}
        </p>
      );
    case "system":
      return (
        <p className="break-words text-neutral-600">
          {event.label}
          {event.detail ? `: ${event.detail}` : ""}
        </p>
      );
    default:
      return (
        <details>
          <summary className="break-words text-neutral-600">{event.label}</summary>
          <pre className={pre}>{event.raw.join("\n")}</pre>
        </details>
      );
  }
}
