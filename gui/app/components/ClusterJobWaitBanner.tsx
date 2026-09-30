// celeris ADR-0090 D5（Phase R7-1）: task が待っているクラスタ job（PBS / Slurm）の 1 行。
// daemon が `poll_secs` ごとに qstat / sacct で確かめ、すべて終われば続きの run を起こす（人の操作は要らない）。
import type { ClusterJobWaitView } from "~/celeris/types";
import { Alert } from "~/components/ui/misc";

/** 「クラスタ job を待っています: 42634 (R) 42635 (Q)」（`status_line` は celeris が組む）。 */
export function clusterJobWaitLine(wait: ClusterJobWaitView): string {
  return `クラスタ job を待っています: ${wait.status_line}`;
}

export function ClusterJobWaitBanner({ wait }: { wait: ClusterJobWaitView }) {
  const next = wait.next_poll_at ? `次の確認 ${wait.next_poll_at}` : "次の確認はまもなく";
  return (
    <section aria-label="クラスタ job の待ち" data-testid="cluster-job-wait-banner">
      <Alert tone="info" icon="clock">
        <p className="font-medium" data-testid="cluster-job-wait-line">
          {clusterJobWaitLine(wait)}
        </p>
        <p className="text-xs text-fg-muted">
          {wait.cluster}（{wait.scheduler}）・{next}・上限 {wait.deadline}
        </p>
      </Alert>
    </section>
  );
}
