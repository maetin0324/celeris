import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { BrowserPendingList, BrowserWait } from "../../api/generated/types";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Section } from "../../components/ui/panel";
import { taskDetailQuery } from "../tasks/task-detail-query";
import {
  BrowserGatewayError,
  browserControlQuery,
  browserKeys,
  browserRunsQuery,
  type ControlStatus,
  type OwnerSession,
  ownerSessionQuery,
} from "./browser-query";
import { type BrowserRunRow, buildRunRows, isActiveRun } from "./browser-runs-model";
import { BrowserWaitsList } from "./browser-waits-panel";
import { OwnerSessionNotice, ownerNoticeReason } from "./owner-session-notice";

// /browser: 実行中・最近の browser run の一覧と未決の待ち（ADR 2026-10-05-browser-department-web-live-view D3.1）。
// raw の live_view_url は扱わない。行から run 画面（同一 origin の Live View）と task 詳細へ link する。

const OPEN_WAITS_PATH = "/api/browser/waits";

export function openBrowserWaitsQuery() {
  return {
    queryKey: [...browserKeys.all, "open-waits"] as const,
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const response = await fetch(OPEN_WAITS_PATH, { credentials: "same-origin", cache: "no-store", signal });
      const value = (await response.json()) as BrowserPendingList & { code?: string };
      if (!response.ok) throw new BrowserGatewayError(response.status, value.code ?? "request_failed");
      return value;
    },
  };
}

export function BrowserRunsScreen() {
  const owner = useQuery(ownerSessionQuery());
  const notice = owner.data ? ownerNoticeReason(owner.data) : null;
  return (
    <ScreenFrame
      title="ブラウザ"
      route="/browser"
      description="ブラウザ実行の状態と、人の対応を待っている依頼を確認します。"
    >
      <div className="mb-4 flex justify-end">
        <Link
          to="/browser/settings"
          className="inline-flex min-h-11 items-center text-link underline underline-offset-2"
        >
          ブラウザ実行課の設定
        </Link>
      </div>
      <FetchFrame query={owner} subject="本人確認">
        {owner.data && notice ? (
          <div data-testid="browser-owner-required" className="space-y-3">
            <OwnerSessionNotice owner={owner.data} />
          </div>
        ) : owner.data ? (
          <OwnerContent owner={owner.data} />
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}

function OwnerContent({ owner }: { owner: OwnerSession }) {
  const runs = useQuery({ ...browserRunsQuery(), refetchInterval: 10000 });
  const waits = useQuery(openBrowserWaitsQuery());
  const items = runs.data?.items ?? [];
  const active = items.filter(isActiveRun);
  const controls = useQueries({
    queries: active.map((run) => ({
      ...browserControlQuery(run.task_id, run.run_id, run.session_id),
      refetchInterval: 10000,
      retry: false,
    })),
  });
  const taskIds = [...new Set(items.map((run) => run.task_id))];
  const tasks = useQueries({ queries: taskIds.map((id) => ({ ...taskDetailQuery(id), retry: false })) });

  const phases = new Map<string, ControlStatus["phase"]>();
  const controlFailed = new Set<string>();
  active.forEach((run, i) => {
    const phase = controls[i]?.data?.status.phase;
    if (phase) phases.set(`${run.task_id}/${run.run_id}`, phase);
    else if (controls[i]?.isError) controlFailed.add(`${run.task_id}/${run.run_id}`);
  });
  const titles = new Map<string, string>();
  for (const item of waits.data?.items ?? []) if (item.task.title) titles.set(item.task.id, item.task.title);
  taskIds.forEach((id, i) => {
    const title = tasks[i]?.data?.task.title;
    if (title) titles.set(id, title);
  });
  const openWaits: BrowserWait[] = (waits.data?.items ?? [])
    .map((item) => item.wait)
    .filter((w) => w.state === "pending");
  const rows = buildRunRows(items, openWaits, { phases, titles, controlFailed });

  return (
    <div className="space-y-6">
      <Section
        title="ブラウザ実行"
        description="実行中のものを先に、人の対応が要るものを一番上に並べます。"
        data-testid="browser-runs"
      >
        <FetchFrame
          query={runs}
          subject="ブラウザ実行"
          empty={items.length === 0}
          emptyMessage="ブラウザ実行はまだありません。browser-enabled の task が走ると、ここに出ます。"
        >
          <ul className="flex flex-col gap-3" aria-label="ブラウザ実行の一覧" data-testid="browser-run-list">
            {rows.map((row) => (
              <RunCard key={`${row.taskId}/${row.runId}`} row={row} />
            ))}
          </ul>
        </FetchFrame>
      </Section>
      <FetchFrame query={waits} subject="ブラウザの待ち">
        {openWaits.length === 0 ? (
          <Section title="ブラウザの人待ち" id="browser-waits" data-testid="browser-waits-empty">
            <EmptyState message="未決の待ちはありません。" />
          </Section>
        ) : (
          <BrowserWaitsList waits={openWaits} owner={owner} />
        )}
      </FetchFrame>
    </div>
  );
}

function RunCard({ row }: { row: BrowserRunRow }) {
  const label = row.taskTitle ?? row.taskId;
  return (
    <li
      className="min-w-0 rounded-lg border border-border bg-surface p-4"
      data-testid="browser-run-row"
      data-task-id={row.taskId}
      data-run-id={row.runId}
    >
      <div className="flex min-w-0 flex-col gap-3 md:flex-row md:items-start md:justify-between">
        <div className="min-w-0 space-y-1">
          <h3 className="break-words text-body font-semibold text-foreground">{label}</h3>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-label">
            <dt className="text-muted-foreground">task</dt>
            <dd className="min-w-0 break-all font-mono">{row.taskId}</dd>
            <dt className="text-muted-foreground">run</dt>
            <dd className="min-w-0 break-all font-mono">{row.runId}</dd>
            <dt className="text-muted-foreground">Live View</dt>
            <dd className="min-w-0 break-words" data-testid="browser-run-live">
              {row.live.label}
            </dd>
          </dl>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2 md:justify-end">
          <Badge tone={row.stateTone} data-testid="browser-run-state">
            {row.stateLabel}
          </Badge>
          {row.lease ? (
            <Badge tone={row.lease.tone} data-testid="browser-run-lease">
              {row.lease.label}
            </Badge>
          ) : row.leasePending ? (
            <span className="text-label text-muted-foreground" data-testid="browser-run-lease">
              {row.leasePending === "unavailable" ? "操作状態を取得できません" : "操作状態を確認中"}
            </span>
          ) : null}
          <Badge tone={row.openWaits > 0 ? "warning" : "neutral"} data-testid="browser-run-waits">
            待ち {row.openWaits} 件
          </Badge>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1">
        <Link
          to="/browser/runs/$taskId/$runId"
          params={{ taskId: row.taskId, runId: row.runId }}
          className="inline-flex min-h-11 items-center text-foreground underline"
          aria-label={`${label} の run ${row.runId} を開く`}
          data-testid="browser-run-open"
        >
          {row.live.available ? "Live View を開く" : "run を開く"}
        </Link>
        <Link
          to="/tasks/$id"
          params={{ id: row.taskId }}
          className="inline-flex min-h-11 items-center text-foreground underline"
          aria-label={`${label} の task 詳細`}
        >
          task 詳細
        </Link>
      </div>
    </li>
  );
}
