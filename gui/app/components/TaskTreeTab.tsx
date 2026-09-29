import { Link } from "react-router";
import type { ActionError } from "~/celeris/action-types";
import type { RollupMetrics2, TaskTreeNode, TaskTreeView, TreeInfo } from "~/celeris/types";
import { Badge, StatusBadge } from "~/components/ui/badge";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { hintClass, touchLinkClass } from "~/components/ui/form";
import { Alert, EmptyState } from "~/components/ui/misc";
import { WORK_UNIT_STATUS_TONE } from "~/lib/task-execution";
import {
  integrationTargetText,
  limitUsageRows,
  progressText,
  rollupCostText,
  runsByRoleText,
  TREE_NODE_PHASE_LABEL,
  TREE_NODE_PHASE_TONE,
  treeNodeHold,
  treeRows,
  unitsByStage,
  wallClockText,
} from "~/lib/tree";
import { cn } from "~/lib/utils";

/**
 * celeris ADR-0079 D14（Phase R4b）: タスク詳細の「木」タブ（`GET /tasks/{id}/task-tree?root=true`）。
 * 節点 = task（root / 子）を前順に、深さで字下げした縦の一覧で出す（モバイル幅でもそのまま。ADR-0055）。
 * 節点ごとに状態・導出値（子待ち・承認待ち・決定待ち・基盤の停止）・止まっている理由・未回答の決定・roll-up
 * （role ごとの run〈reviewer を含む〉・定価・壁時計・leaf の done / total）と、段階ごとの unit（折りたたみ）。
 * root を含む view には木の上限の使用（leaf / run / replan / トークン / 未回答の決定）を出す。
 * 木の無い task（木が無効な設定・旧い task）は 1 節点の木（celeris の応答どおり。タブは隠さない:
 * 自分の run と定価の roll-up はどの task にも意味があるため）。
 * `React.lazy` で読む（タスク詳細の初回 JS の予算、ADR-0055）。
 */
export function TaskTreeTab({
  view,
  error,
  currentId,
  tree,
}: {
  view: TaskTreeView | null;
  error: ActionError | null;
  currentId: string;
  tree: TreeInfo | null | undefined;
}) {
  if (!view) {
    return (
      <EmptyState icon="gitBranch" title="木を読めませんでした" data-testid="task-tree-unavailable">
        {error?.detail ?? "celeris から木を取得できませんでした。"}
      </EmptyState>
    );
  }
  const rows = treeRows(view);
  const byId = new Map(view.nodes.map((n) => [n.id, n]));
  const current = byId.get(currentId);
  const parent = current?.parent_id ? byId.get(current.parent_id) : undefined;
  const target = integrationTargetText(tree, parent?.title ?? null);
  const single = view.nodes.length <= 1;
  return (
    <div className="space-y-4" data-testid="task-tree">
      <Card>
        <CardHeader
          icon="gitBranch"
          title={
            <h2 id="task-tree-heading" className="text-[0.95rem] font-semibold text-fg">
              木の合計
            </h2>
          }
          description={
            single
              ? "この task は子 task を持たない 1 節点の木です（数は自分の分）。"
              : `root から ${view.nodes.length} 個の task（節点）。数は subtree の合計です。`
          }
        />
        <CardBody className="space-y-3 text-sm">
          <RollupLines metrics={view.totals} testId="task-tree-totals" />
          {target && (
            <p className="break-words text-fg-muted" data-testid="task-tree-integration-target">
              {target}
            </p>
          )}
          {!view.tree_enabled && (
            <p className={hintClass} data-testid="task-tree-disabled">
              木の分解（[execution.tree]）は無効です。子 task・決定・計画の承認は作られません。
            </p>
          )}
          {view.limits && <LimitUsage limits={view.limits} />}
        </CardBody>
      </Card>
      <ol className="space-y-2" aria-labelledby="task-tree-heading" data-testid="task-tree-nodes">
        {rows.map(({ node, indent }) => (
          <TreeNodeRow key={node.id} node={node} indent={indent} current={node.id === currentId} />
        ))}
      </ol>
    </div>
  );
}

function RollupLines({ metrics, testId }: { metrics: RollupMetrics2; testId?: string }) {
  return (
    <dl className="grid gap-x-4 gap-y-1 sm:grid-cols-[auto_1fr]" data-testid={testId}>
      <dt className="text-fg-subtle">run</dt>
      <dd className="break-words text-fg" data-testid="rollup-runs">
        {runsByRoleText(metrics)}
        {metrics.runs_in_flight > 0 && `（走っている ${metrics.runs_in_flight}）`}
      </dd>
      <dt className="text-fg-subtle">定価</dt>
      <dd className="break-words text-fg" data-testid="rollup-cost">
        {rollupCostText(metrics)}
        {metrics.reviewer_runs > 0 && `（うち reviewer $${metrics.reviewer_cost_usd.toFixed(2)}）`}
      </dd>
      <dt className="text-fg-subtle">壁時計</dt>
      <dd className="break-words text-fg" data-testid="rollup-wall">
        {wallClockText(metrics)}
      </dd>
      <dt className="text-fg-subtle">進み</dt>
      <dd className="break-words text-fg" data-testid="rollup-progress">
        {progressText(metrics)}
      </dd>
    </dl>
  );
}

function LimitUsage({ limits }: { limits: NonNullable<TaskTreeView["limits"]> }) {
  return (
    <div data-testid="task-tree-limits">
      <p className="font-medium text-fg">木の上限の使用</p>
      <ul className="mt-1 space-y-0.5">
        {limitUsageRows(limits).map((r) => (
          <li
            key={r.key}
            className={cn("break-words", r.near ? "font-semibold text-warning-soft-fg" : "text-fg-muted")}
            data-testid={`task-tree-limit-${r.key}`}
            data-near={r.near ? "true" : undefined}
          >
            {r.label}: {r.used} / {r.max ?? "上限なし"}
            {r.near && "（上限の 8 割以上）"}
          </li>
        ))}
      </ul>
    </div>
  );
}

function TreeNodeRow({ node, indent, current }: { node: TaskTreeNode; indent: number; current: boolean }) {
  const hold = treeNodeHold(node);
  const groups = unitsByStage(node);
  const unitCount = groups.reduce((n, g) => n + g.units.length, 0);
  return (
    <li
      data-testid="task-tree-node"
      data-node-id={node.id}
      data-depth={node.depth}
      data-current={current ? "true" : undefined}
      // モバイル幅でも崩れないよう、字下げは 3 段（2.25rem）で止める（深さは data-depth と「深さ n」で分かる）。
      style={{ marginLeft: `${Math.min(indent, 3) * 0.75}rem` }}
      className={cn(
        "space-y-2 rounded-lg border bg-surface p-3 text-sm shadow-xs",
        current ? "border-primary-border" : "border-border",
        indent > 0 && "border-l-4",
      )}
    >
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge status={node.status} />
        {node.phase && (
          <Badge tone={TREE_NODE_PHASE_TONE[node.phase]} data-testid="task-tree-node-phase">
            {TREE_NODE_PHASE_LABEL[node.phase]}
          </Badge>
        )}
        {node.open_decisions > 0 && (
          <Badge tone="warning" data-testid="task-tree-node-decisions">
            未回答の決定 {node.open_decisions}
          </Badge>
        )}
        <span className="text-fg-subtle">深さ {node.depth}</span>
      </div>
      <p className="break-words font-medium text-fg">
        {current ? (
          <span data-testid="task-tree-node-title">{node.title}（この task）</span>
        ) : (
          <Link
            to={`/tasks/${node.id}?tab=tree`}
            className={cn(touchLinkClass, "text-primary hover:underline")}
            data-testid="task-tree-node-link"
          >
            {node.title}
          </Link>
        )}
      </p>
      {node.parent_unit_key && (
        <p className="break-words text-fg-subtle">
          親の段階「{node.parent_stage ?? "-"}」の unit {node.parent_unit_key}
          {node.plan_version != null && ` ・計画 v${node.plan_version}`}
        </p>
      )}
      {hold && (
        <Alert
          tone={hold.kind === "stall" || hold.kind === "infra" ? "danger" : "warning"}
          data-testid="task-tree-node-hold"
          data-hold-kind={hold.kind}
        >
          <p className="font-semibold">{hold.text}</p>
          {hold.kind === "stall" && <p className="break-words">{hold.detail}</p>}
          {(hold.kind === "decision" || hold.kind === "plan_approval") && (
            <Link to="/inbox" className="inline-flex min-h-11 items-center font-medium underline">
              受信箱で答える
            </Link>
          )}
        </Alert>
      )}
      <RollupLines metrics={node.children && node.children.length > 0 ? node.subtree : node.own} />
      {unitCount > 0 && (
        <details data-testid="task-tree-node-units">
          <summary className="min-h-11 cursor-pointer select-none py-2 text-fg-muted">
            段階ごとの unit（{unitCount}）
          </summary>
          <div className="mt-1 space-y-2">
            {groups.map((g) => (
              <div key={g.stage ?? "-"}>
                {g.stage && <p className="font-medium text-fg-subtle">段階 {g.stage}</p>}
                <ul className="space-y-1">
                  {g.units.map((u) => (
                    <li key={u.key} className="flex flex-wrap items-center gap-x-2 gap-y-1 break-words">
                      <Badge tone={WORK_UNIT_STATUS_TONE[u.status]}>{u.status}</Badge>
                      <span className="font-mono text-fg-subtle">{u.key}</span>
                      {u.child_task_id ? (
                        <Link
                          to={`/tasks/${u.child_task_id}?tab=tree`}
                          className={cn(touchLinkClass, "text-primary hover:underline")}
                        >
                          {u.title}（子 task）
                        </Link>
                      ) : (
                        <span className="text-fg">{u.title}</span>
                      )}
                      {u.blocked_reason && <span className="text-fg-subtle">（{u.blocked_reason}）</span>}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </details>
      )}
    </li>
  );
}
