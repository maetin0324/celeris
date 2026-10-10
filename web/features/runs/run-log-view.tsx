import { useQuery } from "@tanstack/react-query";
import { type ReactNode, type UIEvent, useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { EventRow, EventsPage, RunSummary } from "../../api/generated/types";
import { ConnectionStaleNotice } from "../../components/fetch-state/connection-stale-notice";
import { ErrorNotice, LoadingState, useDelayPhase } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { CodeBlock, LogSurface } from "../../components/ui/code-block";
import { DataList, type DataListItem, DataListRow, DataListTerm, DataListValue } from "../../components/ui/data-list";
import { Icon } from "../../components/ui/icon";
import { Notice } from "../../components/ui/notice";
import { StatusBadge } from "../../components/ui/status-badge";
import { formatAbsolute, serverNowMs } from "../../lib/time";
import { taskDetailQuery } from "../tasks/task-detail-query";
import { eventKindLabel, isNearBottom, runDuration, runHarness, runStatus } from "./run-header";
import { parseRunLog, type RunLogEvent } from "./run-log";
import { useRunLog } from "./use-run-log";

// /tasks/:id/runs/:runId（P3-12、R26）。見出しは取得を待たずに出す（S1）。
// 説明文は置かず、状態などのメタは 1 行（折り返し可）にまとめ、ログ本文を最初の 1 画面に入れる。
// 実行中かどうかは task 詳細の runs の finished_at で決める（分からない間は追い掛ける。取得に失敗したら終わった扱い）。
// 本文は高さ上限のある面の内側でだけ scroll し（ページ全体は広げない・動かさない）、
// 末尾にいるときだけ追記に合わせて面を末尾へ送る。離れていれば「最新へ」を出す。
export function RunLogScreen({ taskId, runId }: { taskId: string; runId: string }) {
  const detail = useQuery(taskDetailQuery(taskId));
  const run = detail.data?.runs.find((item) => item.run_id === runId);
  const running = detail.isError ? false : detail.data ? Boolean(run && !run.finished_at) : undefined;
  const rawLogAvailable = run?.files?.stdout !== false;
  const showRawLog = detail.isError || (Boolean(detail.data) && rawLogAvailable);
  const log = useRunLog(taskId, runId, running, showRawLog);
  const progressQuery = useQuery({
    queryKey: ["tasks", "run-events-view", taskId, runId],
    queryFn: ({ signal }) =>
      apiGet<EventsPage>(
        `/api/tasks/${encodeURIComponent(taskId)}/runs/${encodeURIComponent(runId)}/events?after_seq=0&limit=200`,
        signal,
      ),
    enabled: !rawLogAvailable,
    refetchInterval: running === false ? false : 5000,
  });
  const result = useQuery({
    queryKey: ["tasks", "run-result-view", taskId, runId],
    queryFn: ({ signal }) =>
      apiGet<{ summary?: string | null; question?: string | null }>(
        `/api/tasks/${encodeURIComponent(taskId)}/runs/${encodeURIComponent(runId)}/result`,
        signal,
      ),
    enabled: !rawLogAvailable && run?.files?.result === true,
  });
  const following = log.following;
  const loadingPhase = useDelayPhase(log.status === "loading");
  const events = useMemo(() => parseRunLog(log.buffer.lines), [log.buffer.lines]);
  const [wrap, setWrap] = useState(true);
  const [raw, setRaw] = useState(false);
  return (
    <ScreenFrame
      title={`run ログ ${taskId} / ${runId}`}
      route="/tasks/:id/runs/:runId"
      breadcrumb={[
        { label: `task ${taskId}`, link: { to: "/tasks/$id", params: { id: taskId } } },
        { label: `run ${runId}` },
      ]}
      actions={
        showRawLog ? (
          <>
            <Button size="sm" aria-pressed={wrap} onClick={() => setWrap((v) => !v)}>
              長い行を折り返す
            </Button>
            <Button size="sm" aria-pressed={raw} onClick={() => setRaw((v) => !v)}>
              原文で見る
            </Button>
          </>
        ) : undefined
      }
    >
      <RunHeader run={run} pending={detail.isPending} failed={detail.isError} onRetry={() => void detail.refetch()} />
      <ConnectionStaleNotice />
      {!rawLogAvailable ? (
        <section aria-label="run の進捗と結果" className="flex flex-col gap-3">
          <Notice title="生ログは保存されていません" data-testid="run-no-raw-log">
            この run は機密保護のため生ログ（harness の stdout/stderr）を保存しません。
          </Notice>
          {result.data?.summary || result.data?.question ? (
            <section aria-label="run の結果" className="rounded-lg border border-border bg-surface p-3">
              <h2 className="text-section font-semibold">結果</h2>
              {result.data.summary ? <p className="mt-2 whitespace-pre-wrap">{result.data.summary}</p> : null}
              {result.data.question ? <p className="mt-2 whitespace-pre-wrap">質問: {result.data.question}</p> : null}
            </section>
          ) : null}
          <section
            aria-label="進捗イベント"
            data-testid="run-progress-events"
            className="rounded-lg border border-border bg-surface"
          >
            <ol className="divide-y divide-border">
              {(progressQuery.data?.items ?? [])
                .filter((row) => row.event.type === "worker_progress")
                .map((row: EventRow) => {
                  const e = row.event;
                  if (e.type !== "worker_progress") return null;
                  return (
                    <li key={row.seq} className="p-3">
                      <span className="font-medium">{e.kind ?? "status"}</span>
                      {e.tool ? ` · ${e.tool}` : null}
                      {e.summary || e.msg ? <p className="mt-1 whitespace-pre-wrap">{e.summary ?? e.msg}</p> : null}
                      {e.error ? <span className="text-danger">エラー</span> : null}
                    </li>
                  );
                })}
            </ol>
            {(progressQuery.data?.items ?? []).every((row) => row.event.type !== "worker_progress") ? (
              <p className="p-3 text-muted-foreground">進捗イベントはまだありません。</p>
            ) : null}
          </section>
        </section>
      ) : null}
      {showRawLog ? (
        log.status === "loading" ? (
          <LoadingState
            phase={loadingPhase}
            onRetry={log.retry}
            skeleton={<div aria-hidden="true" className="h-16 animate-pulse rounded-sm bg-muted" />}
          />
        ) : log.status === "error" ? (
          <ErrorNotice subject="run ログ" onRetry={log.retry} />
        ) : (
          <section aria-label="run ログ" className="flex min-w-0 flex-col gap-2" data-testid="run-log">
            <p className="text-label text-muted-foreground">
              <span data-testid="run-log-line-count" data-count={log.buffer.lines.length}>
                {log.buffer.lines.length} 行
              </span>
              {log.buffer.dropped > 0 ? `（古い ${log.buffer.dropped} 行は省略）` : null}
              {following ? "・実行中（追記を追っています）" : null}
            </p>
            {log.capped && running !== false ? (
              <Notice
                title="追記の自動取得を止めました"
                data-testid="run-log-capped"
                action={
                  <Button size="sm" onClick={log.retry}>
                    読み直す
                  </Button>
                }
              >
                長く続いている run のため、一定回数で追うのを止めました。続きは読み直すと表示します。
              </Notice>
            ) : null}
            <FollowScroller version={`${raw}:${log.buffer.lines.length}:${log.buffer.dropped}`} following={following}>
              {log.buffer.lines.length === 0 ? (
                <p
                  data-fetch-state="empty"
                  className="rounded-lg border border-border p-3 text-body text-muted-foreground"
                >
                  まだ出力がありません。{following ? "出力され次第ここに追記します。" : null}
                </p>
              ) : raw ? (
                <LogSurface label="run ログ本文（原文）" wrap={wrap} size="lg" data-follow-target>
                  {log.buffer.lines.join("\n")}
                </LogSurface>
              ) : (
                <EventList events={events} wrap={wrap} />
              )}
            </FollowScroller>
          </section>
        )
      ) : null}
    </ScreenFrame>
  );
}

// header: API にある値（run の状態・harness・開始時刻・所要時間）だけを出す。
// task 詳細を取れなかったとき・run が一覧に無いときは、ログだけを出していることを文字で伝える。
function RunHeader({
  run,
  pending,
  failed,
  onRetry,
}: {
  run: RunSummary | undefined;
  pending: boolean;
  failed: boolean;
  onRetry: () => void;
}) {
  if (pending) return <div aria-busy="true" className="h-12 animate-pulse rounded-sm bg-muted" />;
  if (failed)
    return (
      <Notice
        title="run の概要を取得できませんでした"
        data-testid="run-header-error"
        action={
          <Button size="sm" onClick={onRetry}>
            再試行
          </Button>
        }
      >
        状態・所要時間は出せませんが、ログは読めます。
      </Notice>
    );
  if (!run)
    return (
      <Notice title="この run は task の run 一覧にありません" data-testid="run-header-missing">
        状態・所要時間は出せませんが、ログは読めます。
      </Notice>
    );
  const status = runStatus(run);
  const duration = runDuration(run, serverNowMs());
  const items: DataListItem[] = [
    { label: "状態", value: status ? <StatusBadge status={status} /> : "終了（結果の記録なし）" },
    { label: "harness", value: <span className="break-all">{runHarness(run)}</span> },
    { label: "開始", value: formatAbsolute(run.started_at) },
    { label: "所要時間", value: duration ?? "不明" },
  ];
  return (
    // 項目名と値を横に並べた短い組を 1 行に並べ、幅が足りなければ組ごと折り返す（縦に積まない）。
    <section aria-label="run の概要" data-testid="run-header" className="min-w-0">
      <DataList className="flex flex-wrap gap-x-6 gap-y-1 divide-y-0">
        {items.map((item) => (
          <DataListRow key={String(item.label)} className="flex-row items-center gap-2 py-0 md:gap-2">
            <DataListTerm className="md:w-auto">{item.label}</DataListTerm>
            <DataListValue>{item.value}</DataListValue>
          </DataListRow>
        ))}
      </DataList>
    </section>
  );
}

// 本文の面。scroll するのは子の `data-follow-target` を持つ要素（会話の一覧か、原文の LogSurface）。
// scroll は bubble しないので、capture で面自身の scroll だけを拾う（中の CodeBlock の横 scroll は無視）。
function FollowScroller({
  version,
  following,
  children,
}: {
  version: unknown;
  following: boolean;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const [away, setAway] = useState(false);
  const target = () => ref.current?.querySelector<HTMLElement>("[data-follow-target]") ?? null;
  const onScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    const el = event.target;
    if (!(el instanceof HTMLElement) || !el.hasAttribute("data-follow-target")) return;
    atBottom.current = isNearBottom(el);
    setAway(!atBottom.current);
  }, []);
  // 追記（version の変化）のたびに、末尾にいて追い掛け中なら面の内側だけを末尾へ送る（window は動かさない）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: version は追記と表示切替の合図として使う。
  useLayoutEffect(() => {
    const el = target();
    if (!el) return;
    if (following && atBottom.current) el.scrollTop = el.scrollHeight;
    else setAway(!isNearBottom(el));
  }, [version, following]);
  const toLatest = () => {
    const el = target();
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    atBottom.current = true;
    setAway(false);
  };
  return (
    <div ref={ref} onScrollCapture={onScroll} className="flex min-w-0 flex-col gap-2">
      {children}
      {away ? (
        // 面が画面より高いときも押せるよう、画面の下端に貼り付ける。
        <Button size="sm" className="sticky bottom-4 self-end" onClick={toLatest} data-testid="run-log-latest">
          <Icon name="chevron-down" />
          最新へ
        </Button>
      ) : null}
    </div>
  );
}

function EventList({ events, wrap }: { events: RunLogEvent[]; wrap: boolean }) {
  return (
    <section
      aria-label="run ログ本文"
      // biome-ignore lint/a11y/noNoninteractiveTabindex: scroll 領域を keyboard で読めるようにする（WCAG 2.1.1）。
      tabIndex={0}
      data-follow-target
      className="max-h-screen min-w-0 overflow-y-auto overflow-x-hidden overscroll-contain rounded-lg border border-border bg-surface focus-visible:outline-2 focus-visible:outline-ring"
    >
      <ol className="flex min-w-0 flex-col divide-y divide-border">
        {events.map((event, i) => (
          // 追記は末尾に足すだけなので index で安定する。
          // biome-ignore lint/suspicious/noArrayIndexKey: append-only list
          <li key={i} data-kind={event.kind} className="flex min-w-0 flex-col gap-1 p-3 text-body">
            <EventBody event={event} wrap={wrap} />
          </li>
        ))}
      </ol>
    </section>
  );
}

// event の見出し行。種類は文字の label（Badge）で示し、色だけに頼らない。
function EventHead({ event, children }: { event: RunLogEvent; children?: ReactNode }) {
  const tone = event.kind === "error" ? "danger" : event.kind === "message" ? "info" : "neutral";
  return (
    <div className="flex min-w-0 flex-wrap items-baseline gap-2">
      <Badge tone={tone}>{eventKindLabel(event)}</Badge>
      {children ? <span className="min-w-0 break-words font-mono text-label text-foreground">{children}</span> : null}
    </div>
  );
}

// 開閉できる event（tool・コマンド・思考・未分類）。summary に種類の label と要約を置く。
function Fold({ event, title, children }: { event: RunLogEvent; title?: ReactNode; children?: ReactNode }) {
  if (!children) return <EventHead event={event}>{title}</EventHead>;
  return (
    <details className="group min-w-0">
      <summary className="flex min-h-11 min-w-0 cursor-pointer items-center gap-2 rounded-sm focus-visible:outline-2 focus-visible:outline-ring">
        <Icon name="chevron-right" className="shrink-0 group-open:rotate-90" />
        <EventHead event={event}>{title}</EventHead>
      </summary>
      <div className="mt-1 min-w-0">{children}</div>
    </details>
  );
}

function EventBody({ event, wrap }: { event: RunLogEvent; wrap: boolean }) {
  switch (event.kind) {
    case "message":
      return (
        <>
          <EventHead event={event} />
          <p className="min-w-0 whitespace-pre-wrap break-words text-foreground">{event.text}</p>
        </>
      );
    case "thinking":
      return (
        <Fold event={event} title="思考の内容">
          <p className="min-w-0 whitespace-pre-wrap break-words text-label text-muted-foreground">{event.text}</p>
        </Fold>
      );
    case "tool":
      return (
        <Fold
          event={event}
          title={`${event.name}${event.summary ? `: ${event.summary}` : ""}${event.isError ? "（失敗）" : ""}`}
        >
          {event.result ? (
            <CodeBlock label={`${event.name} の出力`} wrap={wrap} className="max-h-96 overflow-y-auto">
              {event.result}
            </CodeBlock>
          ) : null}
        </Fold>
      );
    case "command":
      return (
        <Fold
          event={event}
          title={`$ ${event.command}${typeof event.exitCode === "number" ? `（exit ${event.exitCode}）` : ""}`}
        >
          {event.output ? (
            <CodeBlock label="コマンドの出力" wrap={wrap} className="max-h-96 overflow-y-auto">
              {event.output}
            </CodeBlock>
          ) : null}
        </Fold>
      );
    case "file_change":
      return <EventHead event={event}>{event.diffs.map((d) => d.path).join(", ")}</EventHead>;
    case "error":
      return (
        <>
          <EventHead event={event} />
          <p className="min-w-0 whitespace-pre-wrap break-words text-foreground">{event.text}</p>
        </>
      );
    case "usage":
      return (
        <EventHead event={event}>
          {event.title}
          {event.stats.length ? ` — ${event.stats.map((s) => `${s.label} ${s.value}`).join("、")}` : ""}
        </EventHead>
      );
    case "system":
      return (
        <EventHead event={event}>
          {event.label}
          {event.detail ? `: ${event.detail}` : ""}
        </EventHead>
      );
    default:
      return (
        <Fold event={event} title={event.label}>
          <CodeBlock label="元の行" wrap={wrap} className="max-h-96 overflow-y-auto">
            {event.raw.join("\n")}
          </CodeBlock>
        </Fold>
      );
  }
}
