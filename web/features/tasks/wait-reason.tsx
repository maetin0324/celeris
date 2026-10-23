import { Link } from "@tanstack/react-router";
import type { HumanGateKind, TaskSummary, TaskWait } from "../../api/generated/types";

function humanGateReason(gate: HumanGateKind, stage: string | null | undefined): string {
  switch (gate) {
    case "plan_approval":
      return "計画の承認待ち";
    case "stage_review":
      return stage ? `段の確認待ち: ${stage}` : "段の確認待ち";
    case "phase_checkpoint":
      return stage ? `途中確認待ち: ${stage}` : "途中確認待ち";
  }
}

export function waitReason(wait: TaskWait): string {
  switch (wait.kind) {
    case "paused":
      return "一時停止中";
    case "ancestor_project_halted":
      return "祖先の案件が停止中";
    case "project_halted":
      return "案件が停止中";
    case "milestone_halted":
      return "途中目標が停止中";
    case "depends_on":
      return `依存待ち: ${wait.task_ids[0] ?? "先行タスク"}`;
    case "approval_parent":
      return "親タスクの承認待ち";
    case "children":
      return `子 ${wait.task_ids.length} 件待ち`;
    case "work_units": {
      const unit = wait.units[0];
      return unit ? `WU 待ち: ${unit.key} ← ${unit.waiting_on[0] ?? unit.waiting_stage ?? "段"}` : "WU 待ち";
    }
    case "stage_halted":
      return `段停止: ${wait.stage}`;
    case "plan_stalled":
      return `計画が詰まっている: ${wait.stage ?? "段"}（${wait.unit_keys.join(", ") || "unit なし"}）`;
    case "human_decision":
      return "人の決定待ち";
    case "human_gate":
      return humanGateReason(wait.gate, wait.stage);
    case "units_blocked":
      return wait.reason === "waiting_for_auth" ? "API キーの登録待ち（登録で自動再開）" : "WU 停止中";
    case "capacity":
      return "枠満杯";
    case "unroutable":
      return "経路なし";
    case "cluster":
      return "cluster 待ち";
    case "write_set":
      return "同じファイルの作業待ち";
    case "backoff":
      return "再試行待ち";
    case "disk_critical":
      return "空き容量の回復待ち";
    case "delegated_children":
      return `委譲した子 ${wait.task_ids.length} 件待ち`;
    case "review_awaiting_human":
      return "人の承認待ち";
    case "depends_on_integration":
      return wait.cause === "delivery_skipped"
        ? `依存先 ${wait.task_id} の取り込みが見送られた: ${wait.detail}`
        : wait.cause === "unmerged"
          ? `依存先 ${wait.task_id} の成果が main に未取り込み`
          : `依存先 ${wait.task_id} は取り込まれない（${wait.cause === "held" ? "保留" : "破棄"}）`;
  }
}

export function waitTargets(wait: TaskWait): string[] {
  switch (wait.kind) {
    case "children":
    case "delegated_children":
    case "depends_on":
      return wait.task_ids;
    case "depends_on_integration":
      return [wait.task_id];
    case "approval_parent":
      return [wait.task_id];
    case "ancestor_project_halted":
      return [wait.task_id];
    case "write_set":
      return wait.blocking_task_ids;
    default:
      return [];
  }
}

export function primaryWait(task: TaskSummary): TaskWait | undefined {
  return task.waits[0];
}

export function TaskWaitReason({ task }: { task: Pick<TaskSummary, "id" | "waits" | "runnable"> }) {
  const wait = task.waits[0];
  if (!wait && !task.runnable) return null;
  if (!wait)
    return (
      <span className="inline-flex rounded-full border border-success-foreground/40 bg-success px-2 py-0.5 text-label text-success-foreground">
        走れる
      </span>
    );
  const targets = waitTargets(wait);
  const detail =
    wait.kind === "work_units" ||
    wait.kind === "stage_halted" ||
    wait.kind === "plan_stalled" ||
    wait.kind === "units_blocked"
      ? `/tasks/${encodeURIComponent(task.id)}#task-tree`
      : wait.kind === "human_decision" || wait.kind === "human_gate" || wait.kind === "review_awaiting_human"
        ? `/tasks/${encodeURIComponent(task.id)}#task-decisions`
        : undefined;
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-label" data-testid="task-wait-reason">
      <span className="break-words text-warning-foreground">{waitReason(wait)}</span>
      {targets.slice(0, 1).map((id) => (
        <Link
          key={id}
          to="/tasks/$id"
          params={{ id }}
          className="inline-flex min-h-11 break-all items-center text-primary underline"
        >
          {id} を開く
        </Link>
      ))}
      {targets.length > 1 ? (
        <Link
          to="/tasks/$id"
          params={{ id: task.id }}
          className="inline-flex min-h-11 items-center text-primary underline"
        >
          ほか {targets.length - 1} 件
        </Link>
      ) : null}
      {detail ? (
        <a href={detail} className="inline-flex min-h-11 items-center text-primary underline">
          詳細
        </a>
      ) : null}
      {task.waits.length > 1 ? <span className="text-muted-foreground">ほか {task.waits.length - 1}</span> : null}
    </span>
  );
}
