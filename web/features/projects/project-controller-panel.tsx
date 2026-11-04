import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet, isApiError } from "../../api/client";
import type { ProjectBrief, ProjectDetail } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, type ActionTarget, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { StatusBadge } from "../../components/ui/status-badge";
import {
  type AuthorityView,
  dismissAction,
  grantFromProposalAction,
  ProjectAuthorityPanel,
  revokeAction,
} from "./project-authority-panel";
import { inputClass } from "./project-list-screen";
import { ProjectRoadmapPanel, type Roadmap, type RoadmapHistory, roadmapDecideAction } from "./project-roadmap-panel";

// ADR 2026-11-04-project-controller A9: 案件の「自動運営」の画面。長期方針・途中目標・関連 root・今後の作業・
// 待機と人の要件・過去の判断と根拠・mode の切り替えを 1 か所に出す。
// 読むのは GET …/controller・…/controller/decisions（A1）、…/roadmap（A5）、…/authority（A7）、案件の詳細と受信箱。
// mode の判定（誰が上げられるか、許可リスト）は API が行い、画面は応答をそのまま出す。

export type ControllerMode = "off" | "dry_run" | "shadow" | "enabled";
export type ControllerPhase = "active" | "waiting" | "blocked" | "paused" | "achieved" | "cancelled";

export type ControllerDecision = {
  id: string;
  mode: ControllerMode;
  proposal_json: string;
  validation: string;
  effect_json: string;
  effect_state: string;
  effect_ref?: string | null;
  created_at: string;
  applied_at?: string | null;
};

export type ControllerView = {
  state: {
    project_id: string;
    mode: ControllerMode;
    phase: ControllerPhase;
    revision: number;
    next_wake_at?: string | null;
    updated_at: string;
    updated_by: string;
  };
  pending_wakes: { id: number; wake_key: string; kind: string; source_ref?: string | null }[];
  last_decision?: ControllerDecision | null;
};

export const CONTROLLER_MODE_ACTION_ID = "project_controller_mode";

const MODE_LABEL: Record<ControllerMode, string> = {
  off: "止めている",
  dry_run: "試しの評価だけ",
  shadow: "観察だけ",
  enabled: "有効",
};

const PHASE_LABEL: Record<ControllerPhase, string> = {
  active: "進行中",
  waiting: "待機中",
  blocked: "人の対応待ちで停止",
  paused: "一時停止",
  achieved: "目標に到達",
  cancelled: "中止",
};

const PHASE_TONE: Record<ControllerPhase, BadgeTone> = {
  active: "running",
  waiting: "info",
  blocked: "warning",
  paused: "warning",
  achieved: "success",
  cancelled: "neutral",
};

const EFFECT_LABEL: Record<string, string> = {
  none: "効果なし",
  pending: "実行待ち",
  applied: "実行済み",
  failed: "失敗",
  suppressed: "抑止（観察のみ）",
};

const WAITING_LABEL: Record<string, string> = { human: "人", task: "task", external: "外部" };

/** mode を置く PUT。`expected_revision` が今の版と違えば API が 409 を返す（画面は再読込する）。 */
export function controllerModeAction(
  projectId: string,
  mode: ControllerMode,
  expectedRevision: number,
  reason: string,
): ActionTarget {
  return {
    id: CONTROLLER_MODE_ACTION_ID,
    method: "PUT",
    path: `/api/projects/${encodeURIComponent(projectId)}/controller`,
    body: { mode, expected_revision: expectedRevision, reason },
  };
}

/** 判断の行の規則の id（`proposal_json.rule_id`）。読めなければ null（行は壊さず出す）。 */
export function decisionRule(decision: ControllerDecision): string | null {
  try {
    const parsed = JSON.parse(decision.proposal_json) as { rule_id?: unknown };
    return typeof parsed.rule_id === "string" ? parsed.rule_id : null;
  } catch {
    return null;
  }
}

/** 案件の root task（新しい順ではなく一覧の順。木の詳細は「途中目標と仕事」が持つ）。 */
export function rootTasks(tasks: ProjectDetail["tasks"]): ProjectDetail["tasks"] {
  return tasks.filter((task) => task.is_root_task || !task.parent_id);
}

function Part({ title, testId, children }: { title: string; testId: string; children: React.ReactNode }) {
  return (
    <section className="min-w-0 space-y-1" data-testid={testId} aria-label={title}>
      <h3 className="text-label font-medium">{title}</h3>
      {children}
    </section>
  );
}

function Empty({ text }: { text: string }) {
  return <p className="text-label text-muted-foreground">{text}</p>;
}

export function ProjectControllerPanel({
  detail,
  brief,
  view,
  decisions,
  pendingHumanItems,
  openAuthorityProposals,
  actionPending,
  onMode,
}: {
  detail: ProjectDetail;
  brief: ProjectBrief | null;
  view: ControllerView;
  decisions: readonly ControllerDecision[];
  pendingHumanItems: number;
  openAuthorityProposals: number;
  actionPending: boolean;
  onMode: (mode: ControllerMode, reason: string) => void;
}) {
  const [reason, setReason] = useState("");
  const { state } = view;
  const roots = rootTasks(detail.tasks);
  const hasReason = reason.trim().length > 0;
  const blocked = state.mode !== "off" && state.phase === "blocked";
  return (
    <div className="min-w-0 space-y-4" data-testid="project-controller">
      <p className="flex flex-wrap items-center gap-2">
        <Badge
          tone={state.mode === "enabled" ? "running" : "neutral"}
          data-testid="controller-mode"
          data-mode={state.mode}
        >
          自動運営: {MODE_LABEL[state.mode] ?? state.mode}
        </Badge>
        {state.mode === "off" ? null : (
          <Badge tone={PHASE_TONE[state.phase] ?? "neutral"} data-testid="controller-phase" data-phase={state.phase}>
            {PHASE_LABEL[state.phase] ?? state.phase}
          </Badge>
        )}
        <span className="text-label text-muted-foreground break-words">
          最後の変更: {state.updated_by}（{state.updated_at}）
        </span>
      </p>

      <Part title="長期方針" testId="controller-policy">
        {brief?.objective ? <p className="break-words">{brief.objective}</p> : null}
        <details>
          <summary className="min-h-11 cursor-pointer py-2">人の依頼文（正本）</summary>
          <Markdown source={detail.project.request} />
        </details>
        <p className="text-label text-muted-foreground">
          依頼文を変えられるのは人だけです。CoS の要約（現状）は依頼文を置き換えません。
        </p>
      </Part>

      <Part title="関連する root task" testId="controller-roots">
        {roots.length === 0 ? (
          <Empty text="root task はまだありません。" />
        ) : (
          <ul className="min-w-0 space-y-1">
            {roots.map((task) => (
              <li key={task.id} data-controller-root={task.id} className="flex flex-wrap items-center gap-2">
                <StatusBadge status={task.status} className="whitespace-nowrap break-normal" />
                <Link
                  className="inline-flex min-h-11 min-w-11 items-center break-words text-primary underline"
                  to="/tasks/$id"
                  params={{ id: task.id }}
                >
                  {task.title}
                </Link>
              </li>
            ))}
          </ul>
        )}
        <p className="text-label text-muted-foreground">
          root task は完了か失敗で終わります。続きは案件の自動運営が次の root task として起票します。
        </p>
      </Part>

      <Part title="今後の作業" testId="controller-next">
        {brief && brief.next_actions.length > 0 ? (
          <ul className="list-disc space-y-1 ps-5">
            {brief.next_actions.map((action) => (
              <li key={action} className="break-words">
                {action}
              </li>
            ))}
          </ul>
        ) : (
          <Empty text="CoS の要約に次の作業はまだありません。" />
        )}
        <p className="text-label text-muted-foreground" data-testid="controller-next-wake">
          次の再評価: {state.next_wake_at ?? "予定なし"} / 未処理のきっかけ: {view.pending_wakes.length} 件
        </p>
      </Part>

      <Part title="待機と人の要件" testId="controller-waiting">
        {blocked ? (
          <p role="alert" className="text-label font-medium" data-testid="controller-blocked">
            自動運営は止まっています。受信箱の判断に答えるか、原因を直してから mode を置き直してください。
          </p>
        ) : null}
        {brief && brief.waiting_on.length > 0 ? (
          <ul className="list-disc space-y-1 ps-5">
            {brief.waiting_on.map((wait) => (
              <li key={`${wait.kind}:${wait.note}`} className="break-words" data-waiting-kind={wait.kind}>
                {WAITING_LABEL[wait.kind] ?? wait.kind}: {wait.note}
                {wait.target ? `（${wait.target}）` : ""}
              </li>
            ))}
          </ul>
        ) : (
          <Empty text="CoS の要約に待ちはありません。" />
        )}
        <p className="text-label">
          <Link className="inline-flex min-h-11 min-w-11 items-center text-primary underline" to="/inbox">
            受信箱の判断待ち {pendingHumanItems} 件
          </Link>
          <span className="ms-3 text-muted-foreground">許可の提案 {openAuthorityProposals} 件（下の「許可」）</span>
        </p>
        <p className="text-label text-muted-foreground">
          人の確認（human check）と本番の操作（release の promote など）は自動運営では進みません。
        </p>
      </Part>

      <Part title="過去の判断と根拠" testId="controller-decisions">
        {decisions.length === 0 ? (
          <Empty text="判断の記録はまだありません。" />
        ) : (
          <ol className="min-w-0 space-y-2">
            {decisions.map((decision) => (
              <li
                key={decision.id}
                className="rounded-md border border-border p-2 break-words"
                data-decision={decision.id}
              >
                <p className="flex flex-wrap items-center gap-2 text-label">
                  <span className="font-medium">{decisionRule(decision) ?? "規則の記録なし"}</span>
                  <Badge tone="neutral" data-effect-state={decision.effect_state}>
                    {EFFECT_LABEL[decision.effect_state] ?? decision.effect_state}
                  </Badge>
                  <span className="text-muted-foreground">{decision.created_at}</span>
                </p>
                <p className="text-label text-muted-foreground">
                  検証: {decision.validation}
                  {decision.effect_ref ? ` / 効果の参照: ${decision.effect_ref}` : ""}
                </p>
              </li>
            ))}
          </ol>
        )}
        {brief && brief.evidence_refs.length > 0 ? (
          <p className="text-label text-muted-foreground break-words" data-testid="controller-evidence">
            CoS が挙げた根拠: {brief.evidence_refs.join("、")}
          </p>
        ) : null}
      </Part>

      <Part title="自動運営の切り替え" testId="controller-mode-change">
        <label className="block space-y-1">
          <span className="text-label">理由（必須）</span>
          <input
            className={inputClass}
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            data-testid="controller-mode-reason"
          />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            disabled={actionPending || !hasReason || state.mode === "shadow"}
            onClick={() => onMode("shadow", reason)}
            data-testid="controller-mode-shadow"
          >
            観察だけにする
          </Button>
          <ConfirmDialog
            trigger={
              <Button
                variant="secondary"
                disabled={actionPending || !hasReason || state.mode === "enabled"}
                data-testid="controller-mode-enabled"
              >
                有効にする
              </Button>
            }
            title="自動運営を有効にする"
            target={detail.project.title}
            consequence="controller がこの案件の root task を起票します。起票の許可（既定 30 日・10 回）を同時に作ります。"
            reversibility="「止める」でいつでも止められます。作成済みの root task は残るので、不要なら別に中止します。"
            followUp="判断の履歴と root task の一覧で確かめてください。"
            confirmLabel="有効にする"
            cancelLabel="やめる"
            onConfirm={async () => onMode("enabled", reason)}
          />
          <Button
            variant="secondary"
            disabled={actionPending || !hasReason || state.mode === "off"}
            onClick={() => onMode("off", reason)}
            data-testid="controller-mode-off"
          >
            止める（従来の CoS に戻す）
          </Button>
        </div>
      </Part>
    </div>
  );
}

async function roadmapOrNull<T>(load: () => Promise<T>): Promise<T | null> {
  try {
    return await load();
  } catch (error) {
    if (isApiError(error) && error.status === 404) return null;
    throw error;
  }
}

/** 詳細画面に載せる入れ物: 取得と操作。表示は `ProjectControllerPanel` と A5・A7 の panel。 */
export function ProjectControllerSection({ detail }: { detail: ProjectDetail }) {
  const projectId = detail.project.id;
  const base = `/api/projects/${encodeURIComponent(projectId)}`;
  const key = projectKeys.detail(projectId);
  const actions = useActionResult(key);
  const controller = useQuery({
    queryKey: [...key, "controller"],
    queryFn: ({ signal }) => apiGet<ControllerView>(`${base}/controller`, signal),
  });
  const decisions = useQuery({
    queryKey: [...key, "controller-decisions"],
    queryFn: ({ signal }) => apiGet<{ items: ControllerDecision[] }>(`${base}/controller/decisions?limit=20`, signal),
  });
  const roadmap = useQuery({
    queryKey: [...key, "roadmap"],
    queryFn: ({ signal }) => roadmapOrNull(() => apiGet<Roadmap>(`${base}/roadmap`, signal)),
  });
  const history = useQuery({
    queryKey: [...key, "roadmap-history"],
    queryFn: ({ signal }) => roadmapOrNull(() => apiGet<RoadmapHistory>(`${base}/roadmap/history`, signal)),
  });
  const authority = useQuery({
    queryKey: [...key, "authority"],
    queryFn: ({ signal }) => apiGet<AuthorityView>(`${base}/authority`, signal),
  });
  const inbox = useQuery(inboxItemsQuery({ project: projectId }));
  const openProposals = authority.data?.proposals.filter((p) => p.state === "open").length ?? 0;
  return (
    <div className="min-w-0 space-y-4">
      {controller.isError ? (
        <p role="alert" data-testid="controller-load-error">
          自動運営の状態を読めませんでした。再読み込みしてください。
        </p>
      ) : null}
      {controller.data ? (
        <ProjectControllerPanel
          detail={detail}
          brief={detail.brief ?? null}
          view={controller.data}
          decisions={decisions.data?.items ?? []}
          pendingHumanItems={inbox.data?.items.length ?? 0}
          openAuthorityProposals={openProposals}
          actionPending={actions.pending}
          onMode={(mode, reason) => {
            void actions.run([controllerModeAction(projectId, mode, controller.data.state.revision, reason)]);
          }}
        />
      ) : null}
      <ActionResultView result={actions.results.project_controller_mode} />
      <ProjectRoadmapPanel
        roadmap={roadmap.data ?? null}
        history={history.data ?? null}
        pending={actions.pending}
        onDecide={(version, decision) => {
          void actions.run([roadmapDecideAction(projectId, version, decision)]);
        }}
      />
      <ActionResultView result={actions.results.project_roadmap_decide} />
      {authority.data ? (
        <ProjectAuthorityPanel
          view={authority.data}
          pending={actions.pending}
          onAccept={(proposal) => {
            void actions.run([grantFromProposalAction(projectId, proposal, proposal.expires_at, proposal.budget)]);
          }}
          onDismiss={(proposal) => {
            void actions.run([dismissAction(projectId, proposal.id)]);
          }}
          onRevoke={(grant) => {
            void actions.run([revokeAction(projectId, grant.id)]);
          }}
        />
      ) : null}
      <ActionResultView result={actions.results.project_authority_grant} />
      <ActionResultView result={actions.results.project_authority_revoke} />
      <ActionResultView result={actions.results.project_authority_dismiss} />
    </div>
  );
}
