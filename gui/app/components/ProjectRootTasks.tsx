import { Link } from "react-router";
import type { ProjectRootTotals, ProjectTaskView, TaskTreeView } from "~/celeris/types";
import { Badge, StatusBadge } from "~/components/ui/badge";
import { Card, CardBody } from "~/components/ui/card";
import { hintClass, touchLinkClass } from "~/components/ui/form";
import { EmptyState, SectionTitle } from "~/components/ui/misc";
import {
  byStatusText,
  progressText,
  rollupCostText,
  runsByRoleText,
  TREE_NODE_PHASE_LABEL,
  TREE_NODE_PHASE_TONE,
  treeNodeHold,
  wallClockText,
} from "~/lib/tree";
import { cn } from "~/lib/utils";

/**
 * celeris ADR-0079 D13 / D14（Phase R4b）: 案件ページの root task の一覧（並列。案件は計画を持たない）。
 * 各 root の状態・導出値・止まっている理由・未回答の決定と、subtree の roll-up（`GET /tasks/{root}/task-tree`
 * の `totals`。引けなかった root は状態だけ）。見出しの下に案件の合計（`GET /projects/{id}` の `root_totals`）。
 * 新しい root task は下の「仕事の木」の「タスクを足す」（`POST /tasks` + `project_id`）で作る。
 */
export function ProjectRootTasks({
  roots,
  trees,
  totals,
}: {
  roots: readonly ProjectTaskView[];
  trees: Readonly<Record<string, TaskTreeView | null>>;
  totals: ProjectRootTotals | null | undefined;
}) {
  return (
    <section aria-labelledby="root-tasks-heading" data-testid="root-tasks-section" className="space-y-4">
      <SectionTitle icon="gitBranch" id="root-tasks-heading" count={roots.length}>
        root task
      </SectionTitle>
      {totals && (
        <p className="break-words text-sm text-fg-muted" data-testid="root-totals">
          {totals.root_tasks} 件（{byStatusText(totals.by_status)}）・{runsByRoleText(totals.totals)}・定価{" "}
          {rollupCostText(totals.totals)}・{progressText(totals.totals)}
        </p>
      )}
      {roots.length === 0 ? (
        <EmptyState icon="gitBranch" title="root task はまだありません">
          下の「仕事の木」の「タスクを足す」か、秘書への依頼で root task を作ります。
        </EmptyState>
      ) : (
        <ul className="space-y-2" data-testid="root-task-list">
          {roots.map((t) => {
            const view = trees[t.id] ?? null;
            const node = view?.nodes[0] ?? null;
            const hold = node ? treeNodeHold(node) : null;
            const openDecisions = view?.totals.open_decisions ?? 0;
            return (
              <li key={t.id} data-testid="root-task-row" data-task-id={t.id}>
                <Card>
                  <CardBody className="space-y-2 text-sm">
                    <div className="flex flex-wrap items-center gap-2">
                      <StatusBadge status={t.status} />
                      {node?.phase && (
                        <Badge tone={TREE_NODE_PHASE_TONE[node.phase]} data-testid="root-task-phase">
                          {TREE_NODE_PHASE_LABEL[node.phase]}
                        </Badge>
                      )}
                      {openDecisions > 0 && (
                        <Badge tone="warning" data-testid="root-task-decisions">
                          未回答の決定 {openDecisions}
                        </Badge>
                      )}
                      {view && view.nodes.length > 1 && (
                        <span className="text-fg-subtle">子 task を含む {view.nodes.length} 節点</span>
                      )}
                    </div>
                    <p className="break-words font-medium">
                      <Link
                        to={`/tasks/${t.id}?tab=tree`}
                        className={cn(touchLinkClass, "text-primary hover:underline")}
                        data-testid="root-task-link"
                      >
                        {t.title}
                      </Link>
                    </p>
                    {hold && (
                      <p
                        className={cn(
                          "break-words font-medium",
                          hold.kind === "stall" || hold.kind === "infra" ? "text-danger" : "text-warning-soft-fg",
                        )}
                        data-testid="root-task-hold"
                      >
                        {hold.text}
                      </p>
                    )}
                    {view ? (
                      <p className="break-words text-fg-muted" data-testid="root-task-rollup">
                        {runsByRoleText(view.totals)}・定価 {rollupCostText(view.totals)}・{progressText(view.totals)}
                        ・壁時計 {wallClockText(view.totals)}
                      </p>
                    ) : (
                      <p className={hintClass}>roll-up は木のタブで見られます。</p>
                    )}
                  </CardBody>
                </Card>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
