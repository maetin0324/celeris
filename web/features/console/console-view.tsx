import { useQuery } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { ConsoleBlock } from "../../api/generated/types";
import { Button } from "../../components/ui/button";
import { fetchRunEvents } from "./run-events";
import { draftStore, expansionStore } from "./store";
import { useConsole, useNewConversation, useSendInstruct } from "./use-console";

// Console の画面（P3-02）。`/` と `/org/:id` が同じ部品を使う。scope は `all` / `node:<id>` / `project:<id>`。

export function normalizeScope(raw: string | undefined): string {
  return raw && /^(project|node):.+/.test(raw) ? raw : "all";
}

function useKeyboardOffset() {
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      const offset = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
      document.documentElement.style.setProperty("--console-kb", `${offset}px`);
    };
    update();
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    return () => {
      vv.removeEventListener("resize", update);
      vv.removeEventListener("scroll", update);
      document.documentElement.style.removeProperty("--console-kb");
    };
  }, []);
}

export function ConsoleView({ scope, label }: { scope: string; label: string }) {
  const console_ = useConsole(scope);
  const send = useSendInstruct(scope);
  const fresh = useNewConversation(scope, console_.cursor);
  const draft = useSyncExternalStore(draftStore.subscribe, () => draftStore.get(scope));
  const composing = useRef(false);
  useKeyboardOffset();

  async function submit() {
    const text = draft.trim();
    if (!text || send.pending || composing.current) return;
    if (await send.send(text)) draftStore.set(scope, "");
  }
  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey) return;
    if (composing.current || event.nativeEvent.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    void submit();
  }

  return (
    <div className="flex flex-col gap-3 pb-40" data-console data-scope={scope}>
      <div className="flex items-center justify-between gap-2">
        <p className="text-sm text-neutral-700">宛先: {label}</p>
        <Button onClick={fresh.start} disabled={fresh.pending}>
          新しい会話
        </Button>
      </div>
      {console_.isError ? (
        <p role="alert" className="text-sm text-red-800">
          Console を読み込めませんでした。
        </p>
      ) : null}
      <ol className="flex flex-col gap-2" aria-label="Console の会話">
        {console_.blocks.map((block) => (
          <li key={blockKey(block)}>
            <Block block={block} />
          </li>
        ))}
      </ol>
      <form
        data-testid="console-composer"
        className="fixed inset-x-0 z-20 border-t border-neutral-300 bg-white px-3 pt-2 md:left-56"
        style={{ bottom: "var(--console-kb, 0px)", paddingBottom: "max(0.5rem, env(safe-area-inset-bottom))" }}
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <div className="flex items-end gap-2">
          <textarea
            aria-label="Console への入力"
            className="min-h-11 flex-1 rounded border border-neutral-400 p-2 text-base"
            rows={2}
            value={draft}
            onChange={(event) => draftStore.set(scope, event.target.value)}
            onKeyDown={onKeyDown}
            onCompositionStart={() => {
              composing.current = true;
            }}
            onCompositionEnd={() => {
              composing.current = false;
            }}
          />
          <Button type="submit" disabled={send.pending || draft.trim() === ""}>
            送信
          </Button>
        </div>
        {send.error ? (
          <p role="alert" className="text-sm text-red-800">
            送信できませんでした。
          </p>
        ) : null}
      </form>
    </div>
  );
}

function blockKey(block: ConsoleBlock): string {
  if (block.kind === "progress") return `progress:${block.progress.run_id}`;
  if (block.kind === "reply" && block.run_id) return `reply:${block.task_id}:${block.run_id}`;
  return `${block.kind}:${block.cursor}`;
}

function Block({ block }: { block: ConsoleBlock }) {
  const base = "rounded border border-neutral-300 p-2 text-sm whitespace-pre-wrap break-words";
  switch (block.kind) {
    case "human":
      return <div className={`${base} bg-neutral-50`}>{block.text}</div>;
    case "reply":
      return <div className={`${base} bg-blue-50`}>{block.text}</div>;
    case "task":
      return (
        <div className={base}>
          タスク {block.task.task_id}: {block.task.title}（{block.task.from} → {block.task.reason}）
        </div>
      );
    case "progress":
      return <Progress block={block} />;
    case "question":
      return <div className={base}>質問: {block.text}</div>;
    case "approval":
      return <div className={base}>承認待ち: {block.approval.id}</div>;
    case "milestone":
      return <div className={base}>途中目標: {block.milestone.title}</div>;
    case "report":
      return <div className={base}>報告: {block.report.headline}</div>;
    case "knowledge":
      return <div className={base}>知識: {block.task_title}</div>;
  }
}

function Progress({ block }: { block: Extract<ConsoleBlock, { kind: "progress" }> }) {
  const id = `progress:${block.progress.run_id}`;
  const expanded = useSyncExternalStore(expansionStore.subscribe, () => expansionStore.has(id));
  const [all, setAll] = useState(false);
  const { task_id: taskId, run_id: runId } = block.progress;
  const events = useQuery({
    queryKey: ["tasks", "run-events", taskId, runId],
    queryFn: ({ signal }) => fetchRunEvents(taskId, runId, signal),
    enabled: all,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const lines = [...block.progress.first, ...block.progress.last];
  return (
    <div className="rounded border border-neutral-300 p-2 text-sm">
      <button
        type="button"
        aria-expanded={expanded}
        className="min-h-11 text-left font-medium"
        onClick={() => expansionStore.toggle(id)}
      >
        {block.title}（tool {block.progress.tool_count} 回）
      </button>
      {expanded ? (
        <div className="mt-1 flex flex-col gap-1">
          <ul>
            {lines.map((line) => (
              <li key={line.seq}>{line.text}</li>
            ))}
          </ul>
          <Button onClick={() => setAll(true)} disabled={all}>
            すべて見る
          </Button>
          {events.isPending && all ? <p role="status">読み込み中</p> : null}
          {events.isError ? <p role="alert">取得できませんでした。</p> : null}
          {events.data ? (
            <ul aria-label="run の全行">
              {events.data.map((line) => (
                <li key={line.seq}>{line.label}</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
