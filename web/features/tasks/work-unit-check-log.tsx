import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { ExecutionWorkUnitView, WorkUnitCheckLog } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { LogSurface } from "../../components/ui/code-block";

export function checkLogQuery(taskId: string, unitId: string) {
  return {
    staleTime: 0,
    queryKey: [...taskKeys.execution(taskId), "check-log", unitId],
    // index を指定せず現在または最後の検査を読む。次の検査への移行も同じ URL で追える。
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<WorkUnitCheckLog>(
        `/api/tasks/${encodeURIComponent(taskId)}/work-units/${encodeURIComponent(unitId)}/check-log`,
        signal,
      ),
  };
}
export function WorkUnitCheckLogs({ taskId, units }: { taskId: string; units: ExecutionWorkUnitView[] }) {
  const checked = units.filter((unit) => unit.check_progress);
  if (checked.length === 0) return null;
  return (
    <section aria-label="WorkUnit の検査ログ" className="min-w-0 space-y-3">
      <h3 className="text-body font-semibold">WorkUnit の検査ログ</h3>
      {checked.map((unit) => (
        <CheckLog key={unit.id} taskId={taskId} unit={unit} />
      ))}
    </section>
  );
}
function CheckLog({ taskId, unit }: { taskId: string; unit: ExecutionWorkUnitView }) {
  const [open, setOpen] = useState(false);
  const running = unit.check_progress?.current != null;
  const query = useQuery({
    ...checkLogQuery(taskId, unit.id),
    enabled: open,
    refetchInterval: open && running ? 5_000 : false,
  });
  const log = query.data;
  return (
    <details
      data-testid="work-unit-check-log"
      className="min-w-0"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="flex min-h-11 cursor-pointer items-center break-words">
        {unit.title} — {running ? "実行中の検査の出力の末尾" : "最後の検査の出力の末尾"}
      </summary>
      {open && (
        <FetchFrame query={query}>
          {log && (
            <div className="min-w-0 space-y-2">
              <p className="break-words text-label">
                検査 {log.index + 1}/{log.total} · {log.cmd} ·{" "}
                {log.running ? "実行中" : log.exit != null ? `exit ${log.exit}` : "終了コードなし"}
                {log.truncated && ` · 末尾のみ（全 ${log.size} バイト）`}
              </p>
              <LogSurface label={`${unit.title} の検査ログ`} wrap size="sm" data-testid="work-unit-check-log-tail">
                {log.tail || "（まだ出力はありません）"}
              </LogSurface>
            </div>
          )}
        </FetchFrame>
      )}
    </details>
  );
}
