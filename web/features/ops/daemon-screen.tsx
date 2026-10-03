import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { DaemonView, ReplayReport } from "../../api/generated/types";
import { daemonKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { daemonPollInterval } from "./daemon-poll";

type Replay = { ok: true; report: ReplayReport } | { ok: false; message: string };

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex flex-wrap gap-2 min-w-0">
      <dt className="font-medium">{label}</dt>
      <dd className="break-words min-w-0">{value}</dd>
    </div>
  );
}

function ReplayPanel() {
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  const [replay, setReplay] = useState<Replay | null>(null);
  async function run() {
    if (pending) return;
    setPending(true);
    try {
      const report = (await apiMutate("POST", "/api/replay", {})) as ReplayReport;
      setReplay({ ok: true, report });
    } catch (error) {
      setReplay({
        ok: false,
        message:
          error instanceof ApiError && (error.kind === "timeout" || error.kind === "network")
            ? "結果を確認できません。再取得して状態を確認してください。"
            : "replay に失敗しました",
      });
    } finally {
      setPending(false);
      await queryClient.invalidateQueries({ queryKey: daemonKeys.rest() });
    }
  }
  return (
    <section className="rounded-lg border border-neutral-300 p-3 space-y-2 min-w-0" aria-label="replay">
      <h2 className="text-lg font-semibold">replay</h2>
      <p className="text-sm">保存された event から状態を再計算し、保存値との差を確認します。</p>
      <Button disabled={pending} onClick={() => void run()}>
        replay を実行
      </Button>
      {replay?.ok === false && (
        <p role="alert" className="text-red-800">
          {replay.message}
        </p>
      )}
      {replay?.ok && (
        <div role="status" className={replay.report.mismatches.length ? "text-amber-900" : "text-green-800"}>
          <p>
            {replay.report.mismatches.length} mismatches across {replay.report.tasks} tasks
          </p>
          <ul className="break-all text-sm">
            {replay.report.mismatches.map((m) => (
              <li key={`${m.task_id}-${m.field}`}>
                {m.task_id} {m.field}: replayed={m.replayed} stored={m.stored}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

function DaemonContent({ view }: { view: DaemonView }) {
  const s = view.snapshot;
  return (
    <div className="space-y-4 min-w-0">
      <section className="rounded-lg border border-neutral-300 p-3 space-y-2 min-w-0" aria-label="状態">
        <h2 className="text-lg font-semibold">状態</h2>
        {!s ? (
          <p>dispatcher の状態はまだありません。</p>
        ) : (
          <dl className="space-y-1">
            <Row label="host" value={`${s.hostname} (pid ${s.pid})`} />
            <Row label="instance" value={s.instance_id} />
            <Row label="開始" value={s.started_at} />
            <Row label="最後の tick" value={s.last_tick_at} />
            <Row label="tick" value={`${s.ticks}（${s.tick_ms} ms）`} />
            <Row label="実行中" value={s.in_flight.length} />
            <Row label="人の待ち" value={s.awaiting_human.length} />
            <Row label="担当なし" value={s.unroutable.length} />
            <Row label="cooldown" value={s.cooldowns.length} />
            <Row label="プロバイダ" value={s.providers.length} />
          </dl>
        )}
      </section>
      <p className="text-sm">取得: {view.now}</p>
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
  return (
    <ScreenFrame title="daemon" route="/daemon">
      <FetchFrame query={query}>{query.data && <DaemonContent view={query.data} />}</FetchFrame>
    </ScreenFrame>
  );
}
