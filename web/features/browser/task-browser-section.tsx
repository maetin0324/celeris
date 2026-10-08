import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { TaskDetail } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { buttonVariants } from "../../components/ui/button";
import { Notice } from "../../components/ui/notice";
import { Section } from "../../components/ui/panel";
import {
  browserControlQuery,
  browserRunsQuery,
  browserWaitsQuery,
  type ControlStatus,
  ownerSessionQuery,
} from "./browser-query";
import { buildRunRows, isActiveRun } from "./browser-runs-model";
import { BrowserWaitsList } from "./browser-waits-panel";
import { OwnerSessionNotice } from "./owner-session-notice";
import { TaskBrowserPolicySection } from "./task-browser-policy";

/** task の概要から browser run と未決の待ちに直接入る。生の live URL は扱わない。 */
export function TaskBrowserSection({ detail }: { detail: TaskDetail }) {
  const taskId = detail.task.id;
  const browserTask = detail.task.skills?.includes("browser-enabled") === true;
  const owner = useQuery({ ...ownerSessionQuery(), enabled: browserTask, retry: false });
  const runs = useQuery({
    ...browserRunsQuery(taskId),
    enabled: browserTask && owner.data?.isOwner === true,
    retry: false,
  });
  const waits = useQuery({ ...browserWaitsQuery(taskId), enabled: browserTask, retry: false });
  const active = (runs.data?.items ?? []).filter(isActiveRun);
  const controls = useQueries({
    queries: active.map((run) => ({
      ...browserControlQuery(taskId, run.run_id, run.session_id),
      retry: false,
    })),
  });
  if (!browserTask) return null;

  const phases = new Map<string, ControlStatus["phase"]>();
  active.forEach((run, index) => {
    const phase = controls[index]?.data?.status.phase;
    if (phase) phases.set(`${taskId}/${run.run_id}`, phase);
  });
  const rows = buildRunRows(runs.data?.items ?? [], waits.data?.items ?? [], { phases });
  const taskWaits = waits.data?.items ?? [];
  return (
    <div className="flex min-w-0 flex-col gap-4" data-testid="task-browser-section">
      <TaskBrowserPolicySection detail={detail} />
      <Section title="ブラウザ実行" description="実行ごとの状態と、人の対応待ちを確認します。">
        {owner.data && !owner.data.isOwner && taskWaits.length === 0 ? <OwnerSessionNotice owner={owner.data} /> : null}
        {owner.isError ? (
          <Notice tone="warning" title="本人確認を取得できません">
            接続を確かめ、画面を読み込み直してください。
          </Notice>
        ) : null}
        {runs.isError ? (
          <Notice tone="warning" title="ブラウザ実行を確認できません">
            本人として登録したセッションで読み込み直してください。
          </Notice>
        ) : null}
        {rows.length > 0 ? (
          <ul className="flex min-w-0 flex-col divide-y divide-border">
            {rows.map((row) => (
              <li
                key={row.runId}
                className="flex min-w-0 flex-wrap items-center gap-2 py-2"
                data-browser-run={row.runId}
              >
                <span className="min-w-0 break-all font-mono text-label" title={row.runId}>
                  run {row.runId}
                </span>
                <Badge tone={row.stateTone}>{row.stateLabel}</Badge>
                {row.lease ? <Badge tone={row.lease.tone}>{row.lease.label}</Badge> : null}
                {row.active && !row.lease ? <Badge tone="neutral">操作状態を確認中</Badge> : null}
                <Badge tone={row.openWaits ? "warning" : "neutral"}>待ち {row.openWaits} 件</Badge>
                <Link
                  to="/browser/runs/$taskId/$runId"
                  params={{ taskId, runId: row.runId }}
                  className={buttonVariants({ variant: "secondary", size: "sm" })}
                >
                  Live View を開く
                </Link>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-label text-muted-foreground">
            {detail.runs.length ? "ブラウザ実行の状態を確認中です。" : "まだブラウザ実行はありません。"}
          </p>
        )}
      </Section>
      {taskWaits.length > 0 ? (
        <BrowserWaitsList waits={taskWaits} owner={owner.data} />
      ) : (
        <Section title="ブラウザの人待ち" id="browser-waits">
          {waits.isError ? (
            <Notice tone="warning" title="待ちを確認できません">
              接続を確かめ、画面を読み込み直してください。
            </Notice>
          ) : (
            <p className="text-label text-muted-foreground">
              {waits.isPending ? "対応待ちを確認中です。" : "現在、対応待ちはありません。"}
            </p>
          )}
        </Section>
      )}
    </div>
  );
}
