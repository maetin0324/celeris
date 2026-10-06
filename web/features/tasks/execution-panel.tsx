import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type {
  ExecutionMode,
  ExecutionPhase,
  TaskDetail,
  TaskExecutionView,
  TaskRoutingView,
} from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { type ActionResult, ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { RoutingAuditView } from "./routing-audit-view";
import { taskDetailQueryKey } from "./task-detail-query";

// /tasks/:id の実行と routing（P3-10、R23）。rereview / promote / phase_gate / execution_decompose、
// routing パネル、execution の表示。execution は taskKeys.execution の key で取り、execution のイベント
// （invalidation-map の E）で取り直す。routing も同じ key の下に置き、同じイベントで取り直す。
// promote と phase_gate は応答と取り直しが済むまで成功と出さない（確定待ちの間は「確定を待っています」）。

const TERMINAL = new Set(["done", "failed", "cancelled"]);

// 実行の段階と形の判定の可視ラベル。写像に無い値は「未確認」（status-badge と同じ扱い）。
const phaseLabel: Readonly<Record<ExecutionPhase, string>> = {
  planning: "計画を作成中",
  executing: "実行中",
  repairing: "修復中",
  verifying: "検証中",
  awaiting_human: "人の判断待ち",
  awaiting_children: "子 task の完了待ち",
  awaiting_plan_approval: "計画の承認待ち",
};

const modeLabel: Readonly<Record<ExecutionMode, string>> = {
  atomic: "一括で実行",
  compound: "段階に分けて実行",
};

function enumLabel<T extends string>(table: Readonly<Record<T, string>>, value: string): string {
  return Object.hasOwn(table, value) ? table[value as T] : "未確認";
}

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
        <p role="status" className="text-label text-muted-foreground">
          確定を待っています
        </p>
      );
    return <ActionResultView result={state?.phase === "done" ? state.result : sender.results[key]} />;
  };

  return (
    <section
      id="execution-panel"
      aria-labelledby="execution-panel-title"
      data-testid="execution-panel"
      className="min-w-0 scroll-mt-4 space-y-3 rounded-lg border border-border bg-surface p-4"
    >
      <h2 id="execution-panel-title" className="text-section font-semibold text-foreground">
        実行と routing
      </h2>

      <FetchFrame query={execution}>
        {execution.data ? (
          <DataList
            data-testid="execution-view"
            items={[
              {
                label: "実行の段階",
                value: execution.data.phase ? (
                  <span data-phase={execution.data.phase}>{enumLabel(phaseLabel, execution.data.phase)}</span>
                ) : (
                  "なし"
                ),
              },
              {
                label: "計画",
                value: execution.data.plan
                  ? `v${execution.data.plan.version}（${execution.data.plan.work_units.length} 件）`
                  : "なし",
              },
              {
                label: "形の判定",
                value: execution.data.gate ? (
                  <span data-mode={execution.data.gate.mode}>{enumLabel(modeLabel, execution.data.gate.mode)}</span>
                ) : (
                  "なし"
                ),
              },
            ]}
          />
        ) : null}
      </FetchFrame>

      <section aria-labelledby="routing-panel-title" data-testid="routing-panel" className="min-w-0 space-y-1">
        <h3 id="routing-panel-title" className="text-body font-semibold text-foreground">
          routing
        </h3>
        <FetchFrame query={routing}>{routing.data ? <RoutingAuditView data={routing.data} /> : null}</FetchFrame>
      </section>

      {has("phase_gate") && (
        <div className="space-y-2" data-testid="phase-gate">
          <h3 className="text-body font-semibold text-foreground">途中確認</h3>
          <label className="block text-label">
            途中確認の note（任意）
            <textarea
              aria-label="途中確認の note（任意）"
              className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
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
          <label className="block text-label">
            成果物の名前
            <input
              className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
              value={promoteName}
              onChange={(event) => setPromoteName(event.target.value)}
            />
          </label>
          <label className="block text-label">
            文書の path
            <input
              className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
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
