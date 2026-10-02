import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { TaskDetail, TaskExecutionView, TaskRoutingView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { type ActionResult, ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";
import { taskDetailQueryKey } from "./task-detail-query";

// /tasks/:id の実行と routing（P3-10、R23）。rereview / promote / phase_gate / execution_decompose、
// routing パネル、execution の表示。execution は taskKeys.execution の key で取り、execution のイベント
// （invalidation-map の E）で取り直す。routing も同じ key の下に置き、同じイベントで取り直す。
// promote と phase_gate は応答と取り直しが済むまで成功と出さない（確定待ちの間は「確定を待っています」）。

const TERMINAL = new Set(["done", "failed", "cancelled"]);

export function executionQuery(taskId: string) {
  return {
    queryKey: taskKeys.execution(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<TaskExecutionView>(`/api/tasks/${encodeURIComponent(taskId)}/execution`, signal),
  };
}

export function routingQuery(taskId: string) {
  return {
    queryKey: [...taskKeys.execution(taskId), "routing"] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<TaskRoutingView>(`/api/tasks/${encodeURIComponent(taskId)}/routing`, signal),
  };
}

type Confirmed = { phase: "waiting" } | { phase: "done"; result: ActionResult };

export function ExecutionPanel({ detail }: { detail: TaskDetail }) {
  const taskId = detail.task.id;
  const base = `/api/tasks/${encodeURIComponent(taskId)}`;
  const queryClient = useQueryClient();
  const sender = useActionResult(taskDetailQueryKey(taskId));
  const execution = useQuery(executionQuery(taskId));
  const routing = useQuery(routingQuery(taskId));
  const [gateNote, setGateNote] = useState("");
  const [promotePath, setPromotePath] = useState("");
  const [promoteName, setPromoteName] = useState("");
  const [confirmed, setConfirmed] = useState<Record<string, Confirmed>>({});
  const has = (action: string) => (detail.actions as string[]).includes(action);

  // 確定まで待つ操作: 応答のあとで detail と execution を取り直し終えてから結果を出す。
  async function runConfirmed(key: string, path: string, body: unknown) {
    setConfirmed((previous) => ({ ...previous, [key]: { phase: "waiting" } }));
    const [result] = await sender.run([{ id: key, path, body }]);
    if (!result) {
      setConfirmed(({ [key]: _, ...rest }) => rest);
      return;
    }
    await queryClient.refetchQueries({ queryKey: taskKeys.execution(taskId) });
    await queryClient.refetchQueries({ queryKey: taskDetailQueryKey(taskId) });
    const final = result.ok ? { ...result, message: "確定しました" } : result;
    setConfirmed((previous) => ({ ...previous, [key]: { phase: "done", result: final } }));
  }

  function simple(key: string, path: string, body: unknown) {
    void sender.run([{ id: key, path, body }]);
  }

  const view = (key: string) => {
    const state = confirmed[key];
    if (state?.phase === "waiting")
      return (
        <p role="status" className="text-neutral-700">
          確定を待っています
        </p>
      );
    return <ActionResultView result={state?.phase === "done" ? state.result : sender.results[key]} />;
  };

  return (
    <section
      aria-labelledby="execution-panel-title"
      data-testid="execution-panel"
      className="min-w-0 space-y-3 rounded border border-neutral-300 p-3"
    >
      <h2 id="execution-panel-title" className="text-base font-semibold">
        実行と routing
      </h2>

      <FetchFrame query={execution}>
        {execution.data ? (
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 text-sm" data-testid="execution-view">
            <dt>実行の段階</dt>
            <dd className="break-words">{execution.data.phase ?? "なし"}</dd>
            <dt>計画</dt>
            <dd className="break-words">
              {execution.data.plan
                ? `v${execution.data.plan.version}（${execution.data.plan.work_units.length} 件）`
                : "なし"}
            </dd>
            <dt>形の判定</dt>
            <dd className="break-words">{execution.data.gate ? execution.data.gate.mode : "なし"}</dd>
          </dl>
        ) : null}
      </FetchFrame>

      <section aria-labelledby="routing-panel-title" data-testid="routing-panel" className="min-w-0 space-y-1">
        <h3 id="routing-panel-title" className="text-sm font-semibold">
          routing
        </h3>
        <FetchFrame query={routing}>
          {routing.data ? (
            <div className="space-y-1 text-sm">
              <p className="break-words">担当 {routing.data.assignee ?? "未定"}</p>
              {routing.data.runs.length === 0 ? (
                <p>routing の記録はありません。</p>
              ) : (
                <ul className="space-y-1">
                  {routing.data.runs.map((run) => (
                    <li key={run.run_id} className="break-words">
                      {run.run_id}: {run.lane ?? "-"} / {run.model ?? "-"} / {run.org_node ?? "-"}
                      {run.rule_id ? `（${run.rule_id}）` : ""}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ) : null}
        </FetchFrame>
      </section>

      {has("phase_gate") && (
        <div className="space-y-2" data-testid="phase-gate">
          <h3 className="text-sm font-semibold">途中確認</h3>
          <label className="block">
            途中確認の note（任意）
            <textarea
              className="block min-h-11 w-full rounded border p-2"
              value={gateNote}
              onChange={(event) => setGateNote(event.target.value)}
            />
          </label>
          <div className="flex flex-wrap gap-2">
            {(
              [
                ["continue", "続ける"],
                ["replan", "計画し直す"],
                ["withdraw", "取り下げる"],
              ] as const
            ).map(([action, label]) => (
              <Button
                key={action}
                disabled={sender.pending}
                onClick={() =>
                  void runConfirmed("phase_gate", `${base}/execution/phase-gate`, { action, note: gateNote || null })
                }
              >
                {label}
              </Button>
            ))}
          </div>
        </div>
      )}
      {view("phase_gate")}

      <div className="flex flex-wrap gap-2">
        {has("rereview") && (
          <Button
            disabled={sender.pending}
            onClick={() => simple("rereview", `${base}/rereview`, { expected_status: detail.task.status })}
          >
            再レビュー
          </Button>
        )}
        {!TERMINAL.has(detail.task.status) &&
          (["atomic", "compound"] as const).map((mode) => (
            <Button
              key={mode}
              disabled={sender.pending}
              onClick={() => simple("execution_decompose", `${base}/execution/decompose`, { mode })}
            >
              {mode === "compound" ? "計画を作らせる" : "1 回で実行する"}
            </Button>
          ))}
      </div>
      {view("rereview")}
      {view("execution_decompose")}

      <details className="min-w-0" data-testid="promote">
        <summary className="inline-flex min-h-11 cursor-pointer items-center">成果物を文書に昇格</summary>
        <div className="space-y-2">
          <label className="block">
            成果物の名前
            <input
              className="block min-h-11 w-full rounded border p-2"
              value={promoteName}
              onChange={(event) => setPromoteName(event.target.value)}
            />
          </label>
          <label className="block">
            文書の path
            <input
              className="block min-h-11 w-full rounded border p-2"
              value={promotePath}
              onChange={(event) => setPromotePath(event.target.value)}
            />
          </label>
          <Button
            disabled={sender.pending || promoteName.trim() === "" || promotePath.trim() === ""}
            onClick={() =>
              void runConfirmed("promote", `${base}/artifacts/promote`, { name: promoteName, path: promotePath })
            }
          >
            昇格する
          </Button>
          {view("promote")}
        </div>
      </details>
    </section>
  );
}
