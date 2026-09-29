import { Link } from "react-router";
import type { ExecutionView, Status, TimelineItem } from "~/celeris/types";
import { Alert } from "~/components/ui/misc";
import { currentStallFromTimeline, taskHold } from "~/lib/tree";

/**
 * celeris ADR-0079 D10 / D14（Phase R4b）: タスク詳細の「止まっている理由」。理由なく止まっている
 * （最後の event が `stall_detected`）ときは「理由なく止まっています」と daemon の分類、名指しの待ち
 * （人の決定・基盤の失敗・計画の承認）のときはその理由と、答える場所（受信箱・木のタブ）へのリンク。
 * 止まっていなければ何も出さない。`React.lazy` で読む（タスク詳細の初回 JS の予算、ADR-0055）。
 */
export function TaskHoldBanner({
  taskId,
  status,
  execution,
  timeline,
}: {
  taskId: string;
  status: Status;
  execution: ExecutionView | null | undefined;
  timeline: readonly TimelineItem[];
}) {
  const hold = taskHold(status, execution, currentStallFromTimeline(timeline, status));
  if (!hold) return null;
  return (
    <section aria-label="止まっている理由" data-testid="task-hold-banner" data-hold-kind={hold.kind}>
      <Alert tone={hold.kind === "stall" || hold.kind === "infra" ? "danger" : "warning"} icon="alert">
        <p className="font-semibold" data-testid="task-hold-text">
          {hold.text}
        </p>
        {hold.kind === "stall" && <p className="break-words">{hold.detail}</p>}
        <p className="flex flex-wrap gap-x-4">
          {hold.kind === "decision" && (
            <Link
              to="/inbox"
              className="inline-flex min-h-11 items-center font-medium underline"
              data-testid="task-hold-inbox-link"
            >
              受信箱の「決定」で答える
            </Link>
          )}
          {hold.kind === "plan_approval" && (
            <Link
              to={`/tasks/${taskId}?tab=overview#execution-mode`}
              className="inline-flex min-h-11 items-center font-medium underline"
              data-testid="task-hold-plan-link"
            >
              「実行の形」で承認する
            </Link>
          )}
          <Link
            to={`/tasks/${taskId}?tab=tree`}
            className="inline-flex min-h-11 items-center font-medium underline"
            data-testid="task-hold-tree-link"
          >
            木で見る
          </Link>
        </p>
      </Alert>
    </section>
  );
}
