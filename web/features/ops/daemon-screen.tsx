import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { DaemonView, Releases, ReplayReport } from "../../api/generated/types";
import { daemonKeys, releaseKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { formatAbsolute, formatRelative } from "../../lib/time";
import { type DaemonLiveness, daemonLiveness, daemonPollExhausted, daemonPollInterval } from "./daemon-poll";

type Replay = { ok: true; report: ReplayReport } | { ok: false; message: string; denied?: boolean };

const livenessView: Record<DaemonLiveness, { tone: BadgeTone; label: string; detail: string }> = {
  running: { tone: "success", label: "稼働中", detail: "最後の tick は 1 分以内です。" },
  stale: { tone: "warning", label: "応答なし", detail: "最後の tick から 1 分以上たっています。" },
  absent: { tone: "neutral", label: "状態なし", detail: "dispatcher の状態はまだありません。" },
};

function Time({ value }: { value: string }) {
  return (
    <time dateTime={value} title={formatAbsolute(value)}>
      {formatRelative(value)}（{formatAbsolute(value)}）
    </time>
  );
}

function ReplayPanel() {
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  const [replay, setReplay] = useState<Replay | null>(null);
  const denied = replay?.ok === false && replay.denied === true;
  async function run() {
    if (pending || denied) return;
    setPending(true);
    try {
      const report = (await apiMutate("POST", "/api/replay", {})) as ReplayReport;
      setReplay({ ok: true, report });
    } catch (error) {
      const forbidden = error instanceof ApiError && error.kind === "forbidden";
      setReplay({
        ok: false,
        denied: forbidden,
        message: forbidden
          ? "権限がありません（403）。replay は実行できないため、ボタンを無効にしました。"
          : error instanceof ApiError && (error.kind === "timeout" || error.kind === "network")
            ? "結果を確認できません。再取得して状態を確認してください。"
            : "replay に失敗しました",
      });
    } finally {
      setPending(false);
      await queryClient.invalidateQueries({ queryKey: daemonKeys.rest() });
    }
  }
  return (
    <Section
      title="replay"
      className="rounded-lg border border-border bg-surface p-4"
      description="保存された event から状態を再計算し、保存値との差を確認します。"
    >
      <div className="space-y-2">
        <Button disabled={pending || denied} onClick={() => void run()}>
          replay を実行
        </Button>
        {replay?.ok === false && (
          <p role="alert" data-testid="replay-error" className="text-danger-foreground">
            {replay.message}
          </p>
        )}
        {replay?.ok && (
          <div role="status" className="space-y-1">
            <p className="flex flex-wrap items-center gap-2">
              <Badge tone={replay.report.mismatches.length ? "warning" : "success"}>
                {replay.report.mismatches.length ? "差あり" : "差なし"}
              </Badge>
              {replay.report.mismatches.length} mismatches across {replay.report.tasks} tasks
            </p>
            <ul className="break-all text-label">
              {replay.report.mismatches.map((m) => (
                <li key={`${m.task_id}-${m.field}`}>
                  {m.task_id} {m.field}: replayed={m.replayed} stored={m.stored}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Section>
  );
}

/** 版: /api/releases の running を読む。読めなければ理由を文字で出す。 */
function ReleaseValue() {
  const releases = useQuery({
    queryKey: releaseKeys.list(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Releases>("/api/releases", signal),
  });
  if (releases.data)
    return <span className="break-all">{`${releases.data.running.release}（${releases.data.running.role}）`}</span>;
  if (releases.isError) {
    const forbidden = releases.error instanceof ApiError && releases.error.kind === "forbidden";
    return (
      <span className="text-muted-foreground">
        {forbidden ? "権限がなく読めません（403）" : "取得できません（/releases で確認してください）"}
      </span>
    );
  }
  return <span className="text-muted-foreground">取得中</span>;
}

function DaemonContent({ view, fetchedAt, exhausted }: { view: DaemonView; fetchedAt: number; exhausted: boolean }) {
  const s = view.snapshot;
  const live = livenessView[daemonLiveness(view.now, s?.last_tick_at)];
  return (
    <div className="min-w-0 space-y-4">
      <Section title="状態" className="rounded-lg border border-border bg-surface p-4">
        <div className="space-y-2">
          <p className="flex flex-wrap items-center gap-2" data-testid="daemon-liveness">
            <Badge tone={live.tone}>{live.label}</Badge>
            <span>{live.detail}</span>
          </p>
          <DataList
            items={[
              { label: "版", value: <ReleaseValue /> },
              {
                label: "最終 poll",
                value: (
                  <span>
                    <Time value={new Date(fetchedAt).toISOString()} />
                    {exhausted ? "。自動更新は上限に達したので止めました。" : "。10 秒ごとに自動で取り直します。"}
                  </span>
                ),
              },
              ...(s
                ? [
                    { label: "最後の tick", value: <Time value={s.last_tick_at} /> },
                    { label: "host", value: <span className="break-all">{`${s.hostname} (pid ${s.pid})`}</span> },
                    { label: "instance", value: <span className="break-all">{s.instance_id}</span> },
                    { label: "開始", value: <Time value={s.started_at} /> },
                    { label: "tick", value: `${s.ticks}（${s.tick_ms} ms）` },
                    { label: "実行中", value: s.in_flight.length },
                    { label: "人の待ち", value: s.awaiting_human.length },
                    { label: "担当なし", value: s.unroutable.length },
                    { label: "cooldown", value: s.cooldowns.length },
                    { label: "プロバイダ", value: s.providers.length },
                  ]
                : []),
            ]}
          />
        </div>
      </Section>
      <p className="text-label text-muted-foreground">取得: {view.now}</p>
      <ReplayPanel />
    </div>
  );
}

export function DaemonScreen() {
  const query = useQuery({
    queryKey: daemonKeys.rest(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<DaemonView>("/api/daemon", signal),
    refetchInterval: (q) => daemonPollInterval(q.state.dataUpdateCount + q.state.errorUpdateCount),
    refetchIntervalInBackground: false,
  });
  const state = useQueryClient().getQueryState(daemonKeys.rest());
  const exhausted = daemonPollExhausted((state?.dataUpdateCount ?? 0) + (state?.errorUpdateCount ?? 0));
  return (
    <ScreenFrame title="daemon" route="/daemon">
      <FetchFrame query={query}>
        {query.data && <DaemonContent view={query.data} fetchedAt={query.dataUpdatedAt} exhausted={exhausted} />}
      </FetchFrame>
    </ScreenFrame>
  );
}
