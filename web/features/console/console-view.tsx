import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { ConsoleBlock } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { CodeBlock, LogSurface } from "../../components/ui/code-block";
import { Icon } from "../../components/ui/icon";
import { formatAbsolute } from "../../lib/time";
import { isNearBottom } from "../runs/run-header";
import { blockKindLabel, blocksVersion, progressPreview, splitFences, stepLine } from "./console-labels";
import { fetchRunEvents } from "./run-events";
import { draftStore, expansionStore } from "./store";
import { useConsole, useNewConversation, useSendInstruct } from "./use-console";

// Console の画面（P3-02）。`/` と `/org/:id` が同じ部品を使う。scope は `all` / `node:<id>` / `project:<id>`。
// 会話は block ごとに種類の文字 label（あなた・返事・run の作業・質問 …）を出し、色だけで区別しない。
// tool の出力・progress の行・本文中の ``` の囲みは CodeBlock/LogSurface（等幅・折り返し）で出し、
// 長い 1 行でもページを広げない。ページ（window）の scroll は、末尾にいるときだけ追記に合わせて末尾へ送り、
// 離れていれば送信欄の上に「最新へ」を出す。

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

function pageMetrics() {
  const el = document.scrollingElement ?? document.documentElement;
  return { scrollTop: el.scrollTop, scrollHeight: el.scrollHeight, clientHeight: window.innerHeight };
}

// 追記の追従。末尾にいる間だけ、version が変わるたびにページの末尾へ送る。離れたら away を立てる。
function useFollowPage(version: string) {
  const atBottom = useRef(true);
  const [away, setAway] = useState(false);
  useEffect(() => {
    const onScroll = () => {
      atBottom.current = isNearBottom(pageMetrics());
      if (atBottom.current) setAway(false);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  useLayoutEffect(() => {
    if (version === "0") return;
    if (atBottom.current) window.scrollTo({ top: pageMetrics().scrollHeight });
    else setAway(true);
  }, [version]);
  const toLatest = useCallback(() => {
    atBottom.current = true;
    setAway(false);
    window.scrollTo({ top: pageMetrics().scrollHeight });
  }, []);
  return { away, toLatest };
}

export function ConsoleView({ scope, label }: { scope: string; label: string }) {
  const console_ = useConsole(scope);
  const send = useSendInstruct(scope);
  const fresh = useNewConversation(scope, console_.cursor);
  const draft = useSyncExternalStore(draftStore.subscribe, () => draftStore.get(scope));
  const composing = useRef(false);
  const follow = useFollowPage(blocksVersion(console_.blocks));
  useKeyboardOffset();

  async function submit() {
    const text = draft.trim();
    if (!text || send.pending || composing.current) return;
    if (await send.send(text)) {
      draftStore.set(scope, "");
      follow.toLatest(); // 自分の発言の後は返事を追う
    }
  }
  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== "Enter" || event.shiftKey) return;
    if (composing.current || event.nativeEvent.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    void submit();
  }

  return (
    <div className="flex min-w-0 flex-col gap-3 pb-40" data-console data-scope={scope}>
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <p className="min-w-0 break-words text-body text-muted-foreground">宛先: {label}</p>
        <Button onClick={fresh.start} disabled={fresh.pending}>
          新しい会話
        </Button>
      </div>
      {console_.isError ? (
        <p role="alert" data-fetch-state="error" className="text-body text-foreground">
          Console を読み込めませんでした。ページを再読み込みすると取り直します。
        </p>
      ) : console_.isPending ? (
        <div aria-busy="true" data-fetch-state="loading" className="h-16 animate-pulse rounded-sm bg-muted" />
      ) : console_.blocks.length === 0 ? (
        <p data-fetch-state="empty" className="rounded-lg border border-border p-3 text-body text-muted-foreground">
          まだ会話がありません。下の入力欄から指示を送ると、返事と作業の様子がここに積み上がります。
        </p>
      ) : null}
      <ol
        className="flex min-w-0 flex-col divide-y divide-border rounded-lg border border-border bg-surface empty:hidden"
        aria-label="Console の会話"
      >
        {console_.blocks.map((block) => (
          <li key={blockKey(block)} data-kind={block.kind} className="flex min-w-0 flex-col gap-2 p-3 text-body">
            <Block block={block} />
          </li>
        ))}
      </ol>
      <form
        data-testid="console-composer"
        className="fixed inset-x-0 z-20 flex flex-col gap-2 border-t border-border bg-background px-3 pt-2 md:left-nav"
        style={{ bottom: "var(--console-kb, 0px)", paddingBottom: "max(0.5rem, env(safe-area-inset-bottom))" }}
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        {follow.away ? (
          <Button type="button" size="sm" className="self-end" onClick={follow.toLatest} data-testid="console-latest">
            <Icon name="chevron-down" />
            最新へ
          </Button>
        ) : null}
        <div className="flex items-end gap-2">
          <textarea
            aria-label="Console への入力"
            className="min-h-11 min-w-0 flex-1 rounded-sm border bg-background p-2 text-base text-foreground focus-visible:outline-2 focus-visible:outline-ring"
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
          <p role="alert" className="text-body text-foreground">
            送信できませんでした。入力は残しています。もう一度送信してください。
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

// block の見出し行: 種類の label（Badge）・誰から/どこへ・時刻。
function BlockHead({ block, children }: { block: ConsoleBlock; children?: ReactNode }) {
  const kind = blockKindLabel(block);
  return (
    <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
      <Badge tone={kind.tone}>{kind.label}</Badge>
      {children ? <span className="min-w-0 break-words text-label text-muted-foreground">{children}</span> : null}
      <time dateTime={block.at} className="ml-auto text-label text-muted-foreground">
        {formatAbsolute(block.at)}
      </time>
    </div>
  );
}

// 人の発言・返事の本文。``` の囲みは CodeBlock、地の文は折り返す。
function Prose({ text, label }: { text: string; label: string }) {
  const parts = splitFences(text);
  if (parts.length === 0) return null;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {parts.map((part, i) =>
        part.kind === "code" ? (
          // biome-ignore lint/suspicious/noArrayIndexKey: 本文から決まる並び
          <CodeBlock key={i} label={`${label}の code`} wrap className="max-h-96 overflow-y-auto">
            {part.text}
          </CodeBlock>
        ) : (
          // biome-ignore lint/suspicious/noArrayIndexKey: 本文から決まる並び
          <p key={i} className="min-w-0 whitespace-pre-wrap break-words text-foreground">
            {part.text}
          </p>
        ),
      )}
    </div>
  );
}

// 開閉できる付属（思考・手順）。summary は 44px の押せる高さ。
function Fold({ title, children }: { title: string; children: ReactNode }) {
  return (
    <details className="group min-w-0">
      <summary className="flex min-h-11 min-w-0 cursor-pointer items-center gap-2 rounded-sm text-label text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring">
        <Icon name="chevron-right" className="shrink-0 group-open:rotate-90" />
        {title}
      </summary>
      <div className="mt-1 min-w-0">{children}</div>
    </details>
  );
}

function Block({ block }: { block: ConsoleBlock }) {
  switch (block.kind) {
    case "human":
      return (
        <>
          <BlockHead block={block}>→ {block.node_id}</BlockHead>
          <Prose text={block.text} label="発言" />
        </>
      );
    case "reply":
      return (
        <>
          <BlockHead block={block}>{block.node_id}</BlockHead>
          {block.thinking ? (
            <Fold title="思考の内容">
              <p className="min-w-0 whitespace-pre-wrap break-words text-label text-muted-foreground">
                {block.thinking}
              </p>
            </Fold>
          ) : null}
          <Prose text={block.text} label="返事" />
          {block.steps && block.steps.length > 0 ? (
            <Fold title={`手順 ${block.steps.length} 件`}>
              <LogSurface label="返事の手順" wrap size="sm">
                {block.steps.map(stepLine).join("\n")}
              </LogSurface>
            </Fold>
          ) : null}
          {block.task_id && block.run_id ? <RunLink taskId={block.task_id} runId={block.run_id} /> : null}
        </>
      );
    case "task":
      return (
        <>
          <BlockHead block={block}>
            {block.task.from} → {block.task.reason}
          </BlockHead>
          <p className="min-w-0 break-words text-foreground">
            <Link
              to="/tasks/$id"
              params={{ id: block.task.task_id }}
              className="font-mono text-label text-foreground underline underline-offset-2"
            >
              {block.task.task_id}
            </Link>{" "}
            {block.task.title}
          </p>
        </>
      );
    case "progress":
      return <Progress block={block} />;
    case "question":
      return (
        <>
          <BlockHead block={block}>{block.node_id ?? undefined}</BlockHead>
          <Prose text={block.text} label="質問" />
          {block.answer ? <Prose text={`回答: ${block.answer}`} label="回答" /> : null}
          <RunLink taskId={block.task_id} runId={block.run_id} />
        </>
      );
    case "approval":
      return (
        <>
          <BlockHead block={block} />
          <p className="min-w-0 break-all font-mono text-label text-foreground">{block.approval.id}</p>
        </>
      );
    case "milestone":
      return (
        <>
          <BlockHead block={block} />
          <p className="min-w-0 break-words text-foreground">{block.milestone.title}</p>
        </>
      );
    case "report":
      return (
        <>
          <BlockHead block={block} />
          <p className="min-w-0 break-words text-foreground">{block.report.headline}</p>
        </>
      );
    case "knowledge":
      return (
        <>
          <BlockHead block={block}>{block.state}</BlockHead>
          <p className="min-w-0 break-words text-foreground">
            {block.task_title}（取り込み {block.ingested}・受信箱 {block.inbox}・破棄 {block.discarded}）
          </p>
        </>
      );
  }
}

function RunLink({ taskId, runId }: { taskId: string; runId: string }) {
  return (
    <Link
      to="/tasks/$id/runs/$runId"
      params={{ id: taskId, runId }}
      className="inline-flex min-h-11 w-fit items-center gap-1 text-label text-foreground underline underline-offset-2"
    >
      run ログを開く（{runId}）
      <Icon name="chevron-right" size="sm" />
    </Link>
  );
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
  const preview = progressPreview(block.progress);
  return (
    <>
      <BlockHead block={block}>
        {[block.assignee, block.harness, block.tier].filter(Boolean).join(" / ") || undefined}
      </BlockHead>
      <button
        type="button"
        aria-expanded={expanded}
        className="flex min-h-11 min-w-0 items-center gap-2 rounded-sm text-left font-medium text-foreground focus-visible:outline-2 focus-visible:outline-ring"
        onClick={() => expansionStore.toggle(id)}
      >
        <Icon name="chevron-right" className={expanded ? "shrink-0 rotate-90" : "shrink-0"} />
        <span className="min-w-0 break-words">
          {block.title}（tool {block.progress.tool_count} 回）
        </span>
      </button>
      {block.progress.last_status ? (
        <p className="min-w-0 break-words text-label text-muted-foreground">{block.progress.last_status}</p>
      ) : null}
      {expanded ? (
        <div className="flex min-w-0 flex-col gap-2">
          {events.data ? (
            <LogSurface label="run の全行" wrap size="md">
              {events.data.map((line) => (line.error ? `（失敗）${line.label}` : line.label)).join("\n")}
            </LogSurface>
          ) : preview.length > 0 ? (
            <LogSurface label="run の先頭と末尾の行" wrap size="sm">
              {preview.join("\n")}
            </LogSurface>
          ) : null}
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Button size="sm" onClick={() => setAll(true)} disabled={all}>
              すべて見る
            </Button>
            <RunLink taskId={taskId} runId={runId} />
          </div>
          {events.isPending && all ? (
            <p role="status" className="text-label text-muted-foreground">
              読み込み中
            </p>
          ) : null}
          {events.isError ? (
            <p role="alert" className="text-label text-foreground">
              取得できませんでした。run ログを開くと全文を読めます。
            </p>
          ) : null}
        </div>
      ) : null}
    </>
  );
}
