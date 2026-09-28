import { type FormEvent, type ReactNode, useState } from "react";
import { Link, useFetcher } from "react-router";
import type { ProjectOpOutcome } from "~/celeris/action-types";
import type {
  MilestoneView,
  PlanDagNode,
  PlanDagProposal,
  ProjectPlanDagView,
  ProjectPlanDecisionInput,
} from "~/celeris/types";
import { ProjectActionFlash } from "~/components/Flash";
import { MarkdownViewer } from "~/components/MarkdownViewer";
import { Badge, StatusBadge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardBody } from "~/components/ui/card";
import { labelClass, textareaClass } from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { Alert } from "~/components/ui/misc";
import { milestoneStatusLabel, milestoneStatusTone } from "~/lib/labels";
import {
  planChangeLabel,
  planLayers,
  planProgressText,
  planQuotaText,
  planStopReasonLabel,
  projectPlanDecisionValid,
} from "~/lib/project-plan";
import { cn } from "~/lib/utils";

/**
 * 案件ページのマイルストーンの DAG（ADR-0074 D3.5、Phase F4b (h)）。
 * 節点 = マイルストーン Task、辺 = `depends_on`（節点の「← 依存先」で示す）。層（依存の最長経路）ごとに
 * デスクトップでは横に並べ、モバイル幅（ADR-0055）では同じ並び（トポロジカル順）の縦の一覧に折り返す
 * （DOM は 1 つで、`lg:` の grid だけが変わる）。節点を選ぶと、その途中目標の「CoS のまとめ → ok / 議論 / ng」
 * （ADR-0038。`renderReview`）をその場で出す。提案中の計画は破線の枠で重ね、「承認 / 却下」を置く。
 */
export function ProjectPlanDag({
  plan,
  milestones,
  renderReview,
}: {
  plan: ProjectPlanDagView;
  milestones: readonly MilestoneView[];
  renderReview: (milestone: MilestoneView) => ReactNode;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const milestoneById = new Map(milestones.map((m) => [m.id, m]));
  const selectedNode = plan.nodes.find((n) => n.key === selected) ?? null;
  const selectedMilestone = selectedNode ? milestoneById.get(selectedNode.milestone_id) : undefined;

  return (
    <div className="space-y-4" data-testid="project-plan-dag">
      {plan.nodes.length > 0 ? (
        <>
          <p className="text-sm text-fg-muted">
            案件計画 version {plan.current_version ?? "-"}。節点を選ぶと、その途中目標の判定（ok / 議論 /
            ng）を出します。
          </p>
          <DagLayers nodes={plan.nodes} selected={selected} onSelect={setSelected} />
          {selectedNode && (
            <Card data-testid="project-plan-selected">
              <CardBody className="space-y-3">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{selectedNode.title}</span>
                  <Link to={`/tasks/${selectedNode.task_id}`} className="text-sm underline underline-offset-2">
                    タスクを開く
                  </Link>
                </div>
                {selectedMilestone &&
                selectedMilestone.status !== "reached" &&
                selectedMilestone.status !== "redesigned" ? (
                  selectedMilestone.review ? (
                    renderReview(selectedMilestone)
                  ) : (
                    <p className="text-sm text-fg-muted" data-testid="project-plan-review-pending">
                      この途中目標の仕事が終わると、CoS がまとめてから判定を仰ぎます。
                    </p>
                  )
                ) : (
                  <p className="text-sm text-fg-muted">判定済みです。</p>
                )}
              </CardBody>
            </Card>
          )}
        </>
      ) : (
        <p className="text-sm text-fg-muted">承認済みの案件計画はまだありません。</p>
      )}
      {plan.pending && <PendingProposal proposal={plan.pending} />}
    </div>
  );
}

function DagLayers({
  nodes,
  selected,
  onSelect,
  dashed = false,
}: {
  nodes: readonly PlanDagNode[];
  selected?: string | null;
  onSelect?: (key: string | null) => void;
  dashed?: boolean;
}) {
  const layers = planLayers(nodes);
  return (
    <ol
      className="space-y-3 lg:grid lg:auto-cols-fr lg:grid-flow-col lg:gap-4 lg:space-y-0"
      data-testid="project-plan-layers"
    >
      {layers.map((layer, i) => (
        // 層は依存の深さで決まる（同じ層の節点は互いに依存しない）。
        // biome-ignore lint/suspicious/noArrayIndexKey: 層の順番そのものが識別子。
        <li key={i} className="space-y-3" data-testid="project-plan-layer">
          <ul className="space-y-3">
            {layer.map((n) => (
              <li key={n.key}>
                <DagNode node={n} dashed={dashed} selected={selected === n.key} onSelect={onSelect} />
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ol>
  );
}

function DagNode({
  node,
  dashed,
  selected,
  onSelect,
}: {
  node: PlanDagNode;
  dashed: boolean;
  selected: boolean;
  onSelect?: (key: string | null) => void;
}) {
  const progress = planProgressText(node);
  const quota = planQuotaText(node.quota);
  return (
    <div
      className={cn(
        "space-y-2 rounded-lg border bg-surface p-3 text-sm",
        dashed ? "border-dashed border-primary-border" : "border-border",
        selected && "ring-2 ring-primary",
      )}
      data-testid="project-plan-node"
      data-node-key={node.key}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{node.title}</span>
        <span className="font-mono text-xs text-fg-subtle">{node.key}</span>
        {node.change && (
          <Badge tone={node.change === "add" ? "success" : node.change === "modify" ? "info" : "warning"}>
            {planChangeLabel(node.change)}
          </Badge>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {node.milestone_status && (
          <Badge
            tone={milestoneStatusTone(node.milestone_status)}
            data-status-badge="milestone"
            data-milestone-status={node.milestone_status}
          >
            {milestoneStatusLabel(node.milestone_status)}
          </Badge>
        )}
        {node.task_status && <StatusBadge status={node.task_status} />}
        {node.stop_reason && (
          <Badge tone={node.stop_reason === "failed" ? "danger" : "warning"} data-testid="project-plan-stop-reason">
            {planStopReasonLabel(node.stop_reason)}
          </Badge>
        )}
      </div>
      {node.depends_on.length > 0 && (
        <p className="text-fg-muted" data-testid="project-plan-edges">
          ← {node.depends_on.join(", ")}
        </p>
      )}
      {(progress || quota) && (
        <p className="text-fg-muted">
          {progress}
          {progress && quota ? "・" : ""}
          {quota && <span data-testid="project-plan-quota">quota {quota}</span>}
        </p>
      )}
      {onSelect && (
        <Button
          type="button"
          variant={selected ? "secondary" : "ghost"}
          size="sm"
          aria-pressed={selected}
          aria-label={`${node.title} の判定を出す`}
          onClick={() => onSelect(selected ? null : node.key)}
          data-testid="project-plan-node-select"
        >
          <Icon name="target" />
          判定
        </Button>
      )}
    </div>
  );
}

function PendingProposal({ proposal }: { proposal: PlanDagProposal }) {
  const fetcher = useFetcher<ProjectOpOutcome>({ key: `project-plan-decide-${proposal.version}` });
  const busy = fetcher.state !== "idle";
  const [note, setNote] = useState("");
  const [invalid, setInvalid] = useState(false);

  function handleSubmit(e: FormEvent<HTMLFormElement>) {
    const submitter = (e.nativeEvent as SubmitEvent).submitter as HTMLButtonElement | null;
    const decision = (submitter?.value ?? "approve") as ProjectPlanDecisionInput;
    if (!projectPlanDecisionValid(decision, note)) {
      e.preventDefault();
      setInvalid(true);
      return;
    }
    setInvalid(false);
  }

  return (
    <div
      className="space-y-3 rounded-lg border-2 border-dashed border-primary-border p-3"
      data-testid="project-plan-pending"
    >
      <p className="font-medium">
        提案中の計画 version {proposal.version}
        {proposal.supersedes != null && `（version ${proposal.supersedes} の見直し）`}
      </p>
      {proposal.rationale && (
        <Alert tone="info" title="CoS の理由">
          <MarkdownViewer content={proposal.rationale} />
        </Alert>
      )}
      <DagLayers nodes={proposal.nodes} dashed />
      <fetcher.Form method="post" onSubmit={handleSubmit} className="space-y-2">
        <input type="hidden" name="intent" value="project_plan_decide" />
        <input type="hidden" name="version" value={proposal.version} />
        <div>
          <label htmlFor={`project-plan-decide-note-${proposal.version}`} className={labelClass}>
            一言（却下は必須。理由は CoS に伝わります）
          </label>
          <textarea
            id={`project-plan-decide-note-${proposal.version}`}
            name="note"
            rows={2}
            value={note}
            onChange={(e) => {
              setNote(e.target.value);
              if (invalid) setInvalid(false);
            }}
            aria-invalid={invalid ? true : undefined}
            className={`${textareaClass} mt-1.5 w-full`}
            data-testid="project-plan-decide-note"
          />
          {invalid && (
            <p
              role="alert"
              className="mt-1 text-sm text-danger lg:text-xs"
              data-testid="project-plan-decide-note-required"
            >
              却下には理由が要ります。
            </p>
          )}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            type="submit"
            name="decision"
            value="approve"
            variant="success"
            size="sm"
            disabled={busy}
            data-testid="project-plan-approve"
          >
            <Icon name="check" />
            承認
          </Button>
          <Button
            type="submit"
            name="decision"
            value="reject"
            variant="danger"
            size="sm"
            disabled={busy}
            data-testid="project-plan-reject"
          >
            <Icon name="x" />
            却下
          </Button>
        </div>
      </fetcher.Form>
      <ProjectActionFlash outcome={fetcher.data} />
    </div>
  );
}
