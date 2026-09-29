import { useEffect, useMemo, useState } from "react";
import { data, isRouteErrorResponse, Link, useFetcher, useNavigate } from "react-router";
import type { ProjectOpOutcome, RetryOutcome, TransitionOutcome } from "~/celeris/action-types";
import { type CelerisClient, getCelerisClient } from "~/celeris/client.server";
import { type CelerisRouteErrorData, celerisErrorResponse } from "~/celeris/errors";
import { formString } from "~/celeris/forms";
import {
  archiveProject,
  cancelProject,
  patchProjectStatus,
  patchProjectText,
  patchProjectWorkspace,
  pauseProject,
  resumeProject,
  unarchiveProject,
} from "~/celeris/projects-admin.server";
import {
  createRepo,
  deleteRepo,
  patchRepo,
  readRepoCreateBody,
  readRepoPatchBody,
  setPrimaryRepo,
} from "~/celeris/repos-admin.server";
import { createTask } from "~/celeris/route-actions.server";
import { buildProjectTaskSpec } from "~/celeris/tasks-admin.server";
import type {
  ArtifactList,
  Clusters,
  ClusterView,
  MilestoneStatus,
  OrgList,
  OrgNode,
  Project,
  ProjectDetail,
  ProjectIntegrationItem,
  ProjectIntegrations as ProjectIntegrationsView,
  ProjectStatus,
  ProjectTaskView,
  ReportList,
  TaskDetail,
  TaskId,
  TaskTreeView,
} from "~/celeris/types";
import { ArtifactsList } from "~/components/ArtifactsList";
import { ErrorFlash, FieldErrors, ProjectActionFlash, RetryFlash } from "~/components/Flash";
import { HelpLink } from "~/components/HelpLink";
import { ProjectIntegrations } from "~/components/ProjectIntegrations";
import { ProjectRepos } from "~/components/ProjectRepos";
import { ProjectRootTasks } from "~/components/ProjectRootTasks";
import { ReportsList } from "~/components/ReportsList";
import { RouteRecovery } from "~/components/RouteRecovery";
import { Badge } from "~/components/ui/badge";
import { Button, buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import {
  checkboxClass,
  hintClass,
  inputClass,
  labelClass,
  selectClass,
  textareaClass,
  touchLinkClass,
} from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { Alert, DataItem, EmptyState, PageHeader, PageToc, SectionTitle } from "~/components/ui/misc";
import type { Tone } from "~/components/ui/tone";
import { WorkspaceFields } from "~/components/WorkspaceFields";
import { WorkTree } from "~/components/WorkTree";
import {
  buildProjectArtifactRows,
  type ProjectArtifactRow,
  type TaskArtifactBundle,
  workspacePlace,
} from "~/lib/artifacts";
import { PRIORITY_LABELS } from "~/lib/board";
import {
  ARCHIVE_CONFIRM_LABEL,
  ARCHIVE_LABEL,
  ARCHIVE_ONLY_TERMINAL_HINT,
  ARCHIVED_BADGE_LABEL,
  CANCEL_CONFIRM_LABEL,
  CANCEL_LABEL,
  CANCEL_STOP_LABEL,
  DOCS_SECTION_DESCRIPTION,
  DOCS_TAB_LABEL,
  milestoneStatusLabel,
  milestoneStatusTone,
  PAUSE_LABEL,
  PROJECT_ARCHIVED_BANNER,
  PROJECT_CANCELLED_BANNER,
  PROJECT_PAUSED_BANNER,
  priorityFullLabel,
  projectArchiveConfirmText,
  projectCancelConfirmText,
  projectStatusLabel,
  RESUME_LABEL,
  TASK_CATEGORIES,
  TIERS,
  taskCategoryLabel,
  taskStatusLabel,
  tierLabel,
  UNARCHIVE_LABEL,
} from "~/lib/labels";
import { projectIsArchived, projectIsPaused, projectLifecycleButtons } from "~/lib/lifecycle";
import { isTransientStatus } from "~/lib/recovery";
import { revalidateAfterActionErrors } from "~/lib/revalidate";
import { projectRootTasks } from "~/lib/tree";
import { cn } from "~/lib/utils";
import { projectTasksToGraph, visibleWorkTasks } from "~/lib/work-tree";
import { readWorkspaceFromForm, workspaceKindOf, workspaceSummaryText } from "~/lib/workspace-form";
import { CelerisBanner } from "~/root";
import type { Route } from "./+types/projects.$id";

/**
 * `/projects/:id`（案件の詳細・途中目標・仕事の木・報告・成果物、SPEC §3.3・§3.5・§3.7、ADR-0033 D2/D3、
 * docs/gui/api.md §3.47〜3.51、Phase G13c）。
 * 「仕事の木」は `GET /projects/{id}` の `tasks`（`ProjectTaskView`、`parent_id` / `depends_on` は既存の DAG
 * と同じ辺の作り方）を `/graph` と同じ `layoutGraph`（`~/components/WorkTree.tsx`）で描く。
 * 各ノードには `assignee` の組織ノードの名前を出す（`GET /org` と突き合わせる。組織のノード名を出すだけで、
 * celeris 側の判断値は増やさない）。
 * 「報告」タブは `GET /reports?project=<id>`（**全レベル**。`level` を付けない。`/reports` の既定は秘書
 * レベルの未読だけだが、案件詳細ではこの案件のすべての段の報告を見せる。G13b-1 の依頼どおり）を
 * `/reports` と同じ `ReportsList` で出す。
 * 「成果物」節は `~/routes/artifacts.tsx`（横断一覧）と同じ組み立て（`~/lib/artifacts.ts::buildProjectArtifactRows`。
 * celeris への問い合わせ自体は各 loader に閉じる私的ヘルパー。下記コメント参照）で、この案件のタスクぶんだけを
 * `~/components/ArtifactsList.tsx` で出す（G13b-1 の「報告」タブと同じ作り）。
 */

export interface ProjectDetailData {
  detail: ProjectDetail;
  org: OrgList;
  reports: ReportList;
  artifactRows: ProjectArtifactRow[];
  fetchedAt: string;
  /** 作業場所の編集フォームの選択肢（`GET /clusters`。ADR-0039 D1、Phase G13k）。celeris に届かないときは空。 */
  clusters: ClusterView[];
  /**
   * 「PR と取り込み」節（`GET /projects/{id}/integrations`。ADR-0043 D5、Phase 54 / G18）。
   * タスク × リポジトリごとに最新の 1 件を celeris が新しい順で返す。落ちても案件の詳細自体は出す。
   */
  integrations: ProjectIntegrationItem[];
  /**
   * celeris ADR-0079 D14（Phase R4b）: root task ごとの木（`GET /tasks/{root}/task-tree`。subtree の roll-up・導出値）。
   * 引けなかった root と、`ROOT_TREE_FETCH_LIMIT` を超えた分は `null`（一覧には状態だけ出す）。
   */
  rootTrees: Record<string, TaskTreeView | null>;
}

/** 案件ページで木を引く root task の上限（root ごとに節点の events を読むため。超えた分は木のタブで見る）。 */
const ROOT_TREE_FETCH_LIMIT = 20;

/**
 * 1 タスクぶんの成果物 + 置き場所を束ねる（N+1。`GET /tasks/{id}` と `GET /tasks/{id}/artifacts`）。
 * `~/routes/artifacts.tsx` に同じ形の私的ヘルパーがある（React Router のクライアントバンドル除去は
 * `loader`/`action` 等に限られるため、公開関数から `.server.ts` モジュールを参照しない。重複はこの小ささでは許容する）。
 */
async function loadTaskArtifactBundles(
  client: CelerisClient,
  taskIds: readonly TaskId[],
  signal: AbortSignal | undefined,
): Promise<Map<TaskId, TaskArtifactBundle>> {
  const entries = await Promise.all(
    taskIds.map(async (id) => {
      const [detail, list] = await Promise.all([
        client.get<TaskDetail>(`/tasks/${encodeURIComponent(id)}`, { signal }).catch(() => null as TaskDetail | null),
        client
          .get<ArtifactList>(`/tasks/${encodeURIComponent(id)}/artifacts`, { signal })
          .catch(() => ({ items: [] }) as ArtifactList),
      ]);
      const bundle: TaskArtifactBundle = {
        workspace: detail
          ? workspacePlace(detail.task.workspace, detail.workspace_dir)
          : { text: "-", vscodeHref: null, localCopyNote: null },
        artifacts: list.items,
      };
      return [id, bundle] as const;
    }),
  );
  return new Map(entries);
}

export async function loadProjectDetail(
  client: CelerisClient,
  id: string,
  request: Request,
): Promise<ProjectDetailData> {
  const [detail, org, reports, clusters, integrations] = await Promise.all([
    client.get<ProjectDetail>(`/projects/${encodeURIComponent(id)}`, { signal: request.signal }),
    client.get<OrgList>("/org", { signal: request.signal }).catch(() => ({ items: [] }) as OrgList),
    client
      .get<ReportList>("/reports", { query: { project: id }, signal: request.signal })
      .catch(() => ({ items: [] }) as ReportList),
    // 作業場所の編集フォームの選択肢（ADR-0039 D1、Phase G13k）。落ちても案件の詳細自体は出す。
    client.get<Clusters>("/clusters", { signal: request.signal }).catch(() => ({ items: [] }) as Clusters),
    // 取り込みの記録（ADR-0043 D5、Phase 54 / G18）。落ちても案件の詳細自体は出す。
    client
      .get<ProjectIntegrationsView>(`/projects/${encodeURIComponent(id)}/integrations`, { signal: request.signal })
      .catch(() => ({ items: [] }) as ProjectIntegrationsView),
  ]);
  const orgById = new Map(org.items.map((n) => [n.id, n]));
  const bundles = await loadTaskArtifactBundles(
    client,
    detail.tasks.map((t) => t.id),
    request.signal,
  );
  const artifactRows = buildProjectArtifactRows(detail.tasks, bundles, orgById);
  const rootTrees: Record<string, TaskTreeView | null> = {};
  const roots = projectRootTasks(detail.tasks);
  await Promise.all(
    roots.slice(0, ROOT_TREE_FETCH_LIMIT).map(async (t) => {
      rootTrees[t.id] = await client
        .get<TaskTreeView>(`/tasks/${encodeURIComponent(t.id)}/task-tree`, { signal: request.signal })
        .catch(() => null);
    }),
  );
  return {
    detail,
    org,
    reports,
    artifactRows,
    fetchedAt: new Date().toISOString(),
    clusters: clusters.items,
    integrations: integrations.items,
    rootTrees,
  };
}

export const shouldRevalidate = revalidateAfterActionErrors;

export async function loader({ params, request }: Route.LoaderArgs): Promise<ProjectDetailData> {
  try {
    return await loadProjectDetail(getCelerisClient(), params.id, request);
  } catch (e) {
    throw celerisErrorResponse(e);
  }
}

export function meta(_: Route.MetaArgs) {
  return [{ title: "案件詳細 - Celeris" }];
}

export async function action({ request, params }: Route.ActionArgs) {
  const form = await request.formData();
  const intent = form.get("intent");
  const client = getCelerisClient();

  let outcome: ProjectOpOutcome;
  switch (intent) {
    case "project_status":
      outcome = await patchProjectStatus(
        client,
        params.id,
        (formString(form, "status") ?? "proposed") as ProjectStatus,
        request.signal,
      );
      break;
    // celeris ADR-0072「Phase F6 実装時の決定」: 案件の名前・説明（依頼文）・slug の編集。
    case "project_edit":
      outcome = await patchProjectText(client, params.id, form, request.signal);
      break;
    // celeris ADR-0079 D13（Phase R5a）: 案件計画（`project_plan` / `project_plan_decide`）と途中目標の書き込み
    // （`milestone_create` / `milestone_status` / `milestone_decide` / `milestone_cancel|pause|resume`）は celeris が
    // 410 を返すので中継を外した（下の default で 400 になる）。
    // 作業場所の保存・消去（ADR-0039 D1、Phase G13k）。保存は選んだ kind（local/remote）をそのまま送り、
    // 消去は明示的に `workspace: null` を送る（別ボタン。編集フォームで「まだ決めない」は選べない）。
    case "project_workspace_save":
      outcome = await patchProjectWorkspace(client, params.id, readWorkspaceFromForm(form), request.signal);
      break;
    case "project_workspace_clear":
      outcome = await patchProjectWorkspace(client, params.id, null, request.signal);
      break;
    // 案件のリポジトリ（ADR-0043 D1、docs/celeris-api-v1.md §3.69〜3.71。Phase 52 / G16）。
    // どれもフォームの値を対応する要求に写すだけで、GUI 側では検証しない（409 / 422 は celeris の文言）。
    case "repo_create":
      outcome = await createRepo(client, params.id, readRepoCreateBody(form), request.signal);
      break;
    case "repo_patch":
      outcome = await patchRepo(client, formString(form, "repo_id") ?? "", readRepoPatchBody(form), request.signal);
      break;
    case "repo_primary":
      outcome = await setPrimaryRepo(client, formString(form, "repo_id") ?? "", request.signal);
      break;
    case "repo_delete":
      outcome = await deleteRepo(client, formString(form, "repo_id") ?? "", request.signal);
      break;
    // ADR-0044 D1（Phase 53）: 案件・途中目標から人がタスクを足す。人が作ったタスクは `ready`
    // （`POST /tasks` の既定。`status` は送らない）。
    case "task_create": {
      const created = await createTask(client, buildProjectTaskSpec(form, params.id), request.signal);
      outcome = created.ok ? { ok: true, op: "task_create", task: created.task } : { ...created, op: "task_create" };
      break;
    }
    // 中止・一時停止・アーカイブ（ADR-0044 D6、docs/celeris-api-v1.md §3.84〜3.91。Phase 55 / G19）。
    // どれも本文は `{}` で、できるかどうかは celeris が決める（409 `invalid_transition` はそのまま出す）。
    case "project_cancel":
      outcome = await cancelProject(client, params.id, request.signal);
      break;
    case "project_pause":
      outcome = await pauseProject(client, params.id, request.signal);
      break;
    case "project_resume":
      outcome = await resumeProject(client, params.id, request.signal);
      break;
    case "project_archive":
      outcome = await archiveProject(client, params.id, request.signal);
      break;
    case "project_unarchive":
      outcome = await unarchiveProject(client, params.id, request.signal);
      break;
    default:
      throw data({ error: `unknown intent: ${String(intent)}` }, { status: 400 });
  }
  return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
}

// 「状態を直接変える」プルダウンの選択肢。`paused` / `cancelled` は専用のボタン（ADR-0044 D6）で
// 行う（`PATCH` では連鎖も `paused_from` も起きないので、ここには並べない）。
const PROJECT_STATUSES: ProjectStatus[] = ["proposed", "active", "done"];

const PROJECT_STATUS_TONE: Record<ProjectStatus, Tone> = {
  proposed: "info",
  active: "primary",
  paused: "warning",
  done: "success",
  cancelled: "neutral",
};

export default function ProjectDetailPage({ loaderData }: Route.ComponentProps) {
  const { detail, org, reports, artifactRows, fetchedAt, clusters, integrations, rootTrees } = loaderData;
  const { project, milestones, tasks } = detail;
  // ADR-0043 D1（Phase 52 / G16）: 並びは celeris が決めたもの（primary が先頭）をそのまま使う。
  const repos = detail.repos ?? [];
  const fetcher = useFetcher<ProjectOpOutcome>();
  const submitting = fetcher.state !== "idle";
  const workspaceError = fetcher.data && !fetcher.data.ok ? fetcher.data.error : undefined;

  const orgById = useMemo(() => new Map(org.items.map((n) => [n.id, n])), [org.items]);
  // 裏方のタスク（`support`: 対話・報告のまとめ・承認待ち・レビュー。Phase 29）は仕事の木から完全に外す
  // （SPEC「タスクは裏方」/ ADR-0033 D8）。件数表示・「担当に話す」一覧（下の `work-tree-assignees`）も
  // 同じ判断に揃えるため、ここで 1 度だけ絞る。
  const workTasks = useMemo(() => visibleWorkTasks(tasks), [tasks]);
  const graph = useMemo(() => projectTasksToGraph(tasks, orgById), [tasks, orgById]);
  const roots = useMemo(() => projectRootTasks(tasks), [tasks]);

  return (
    <div className="space-y-8">
      <PageHeader
        icon="folder"
        title={
          <>
            {project.title}
            <HelpLink anchor="screens" label="画面ごとの説明" />
          </>
        }
        description="案件は組織の上から入り、分解されて下へ流れます。その依存関係が「仕事の木」です。"
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone={PROJECT_STATUS_TONE[project.status]} data-testid="project-status" data-status-badge="project">
              {projectStatusLabel(project.status)}
            </Badge>
            {/* ADR-0044 D6（Phase 55 / G19）: 一時停止・中止・アーカイブは題名の横でも分かるようにする
                （状態のバッジと重なるが、アーカイブは `status` に出ないので別に要る）。 */}
            {projectIsPaused(project) && (
              <Badge tone="warning" data-testid="project-paused-badge">
                {PAUSE_LABEL}
              </Badge>
            )}
            {project.status === "cancelled" && (
              <Badge tone="neutral" data-testid="project-cancelled-badge">
                {CANCEL_LABEL}
              </Badge>
            )}
            {projectIsArchived(project) && (
              <Badge tone="neutral" data-testid="project-archived-badge">
                {ARCHIVED_BADGE_LABEL}
              </Badge>
            )}
          </div>
        }
      />

      <ProjectLifecycleActions project={project} />

      {/* 止まっている案件は、押しても新しい仕事が始まらないことを画面の上で必ず言う（ADR-0044 D6）。 */}
      {projectIsPaused(project) && (
        <Alert tone="warning" data-testid="project-paused-banner">
          {PROJECT_PAUSED_BANNER}
        </Alert>
      )}
      {project.status === "cancelled" && (
        <Alert tone="warning" data-testid="project-cancelled-banner">
          {PROJECT_CANCELLED_BANNER}
        </Alert>
      )}
      {projectIsArchived(project) && (
        <Alert tone="info" data-testid="project-archived-banner">
          {PROJECT_ARCHIVED_BANNER}
        </Alert>
      )}

      <ProjectActionFlash outcome={fetcher.data} />

      {/* Phase 95（ADR-0055 ラウンド 19、所見 P-project-detail、重さ「高」）: このページは性質の違う
          9 節（依頼・作業場所・リポジトリ・PR と取り込み・途中目標・この方針で進める・仕事の木・報告・
          成果物・文書）が縦に並ぶ、スマホでは 6 画面分を超える長い 1 ページ。`/help` と同じ「目次から
          飛ぶ」パターン（`PageToc`）を足し、節そのもの（`SectionTitle` の id）には触れない。 */}
      <PageToc
        label="案件詳細の目次"
        items={[
          { id: "project-detail-heading", icon: "file", label: "依頼" },
          { id: "root-tasks-heading", icon: "gitBranch", label: "root task" },
          { id: "project-workspace-heading", icon: "folder", label: "作業場所" },
          { id: "project-repos-heading", icon: "database", label: "リポジトリ" },
          { id: "project-integrations-heading", icon: "gitBranch", label: "PR と取り込み" },
          ...(milestones.length > 0
            ? [{ id: "milestones-heading", icon: "target" as const, label: "以前の途中目標" }]
            : []),
          { id: "project-plan-heading", icon: "sparkles", label: "この方針で進める" },
          { id: "work-tree-heading", icon: "gitBranch", label: "仕事の木" },
          { id: "project-reports-heading", icon: "send", label: "報告" },
          { id: "project-artifacts-heading", icon: "file", label: "成果物" },
          { id: "project-docs-heading", icon: "book", label: DOCS_TAB_LABEL },
        ]}
      />

      <section aria-labelledby="project-detail-heading" className="space-y-4">
        <SectionTitle icon="file" id="project-detail-heading">
          依頼
        </SectionTitle>
        <Card>
          <CardBody className="space-y-4">
            <DataItem label="依頼文" wide>
              <p className="whitespace-pre-wrap" data-testid="project-request-text">
                {project.request}
              </p>
            </DataItem>
            <ProjectEditForm project={project} />
            {project.secretary_summary && (
              <Alert tone="info" title="CoS の理解の確認・方針" data-testid="project-secretary-summary">
                <p className="whitespace-pre-wrap">{project.secretary_summary}</p>
              </Alert>
            )}
            <fetcher.Form method="post" className="flex flex-wrap items-end gap-3" data-testid="project-status-form">
              <input type="hidden" name="intent" value="project_status" />
              <div>
                <label htmlFor="project-status-select" className={labelClass}>
                  案件の状態
                </label>
                <select
                  id="project-status-select"
                  name="status"
                  defaultValue={project.status}
                  className={`${selectClass} mt-1.5`}
                >
                  {PROJECT_STATUSES.map((s) => (
                    <option key={s} value={s}>
                      {projectStatusLabel(s)}
                    </option>
                  ))}
                </select>
              </div>
              <Button
                type="submit"
                variant="secondary"
                size="sm"
                disabled={submitting}
                data-testid="project-status-submit"
              >
                <Icon name="check" />
                状態を変える
              </Button>
            </fetcher.Form>
          </CardBody>
        </Card>
      </section>

      {/* 案件の作業場所（ADR-0039 D1、Phase G13k）。コードを扱う仕事の子タスクが継ぐ場所
          （明示 > 案件 > 親。ADR-0039 D2）で、未設定だと空の作業ディレクトリで走ってしまう
          （実機の事故 2026-09-18）。 */}
      {/* celeris ADR-0079 D13 / D14（Phase R4b）: root task の一覧（並列、subtree の roll-up 付き）。 */}
      <ProjectRootTasks roots={roots} trees={rootTrees ?? {}} totals={detail.root_totals} />

      <section aria-labelledby="project-workspace-heading" data-testid="project-workspace" className="space-y-4">
        <SectionTitle icon="folder" id="project-workspace-heading">
          作業場所
        </SectionTitle>
        <Card>
          <CardBody className="space-y-4">
            {project.workspace ? (
              <DataItem label="現在" wide>
                <p className="break-all font-mono text-sm">{workspaceSummaryText(project.workspace)}</p>
              </DataItem>
            ) : (
              <Alert tone="warning" data-testid="project-workspace-unset">
                未設定 — コードを扱う仕事は空の作業ディレクトリで走ります
              </Alert>
            )}
            <fetcher.Form method="post" className="space-y-3" data-testid="project-workspace-form">
              <input type="hidden" name="intent" value="project_workspace_save" />
              <WorkspaceFields
                idPrefix="project-workspace"
                clusters={clusters}
                allowUndecided={false}
                defaultKind={project.workspace ? workspaceKindOf(project.workspace) : "local"}
                defaultPath={project.workspace?.path ?? ""}
                defaultCluster={project.workspace?.kind === "remote" ? project.workspace.cluster : undefined}
                error={workspaceError}
              />
              <div className="flex flex-wrap gap-2">
                <Button
                  type="submit"
                  variant="secondary"
                  size="sm"
                  disabled={submitting}
                  data-testid="project-workspace-save"
                >
                  <Icon name="check" />
                  保存
                </Button>
              </div>
            </fetcher.Form>
            <fetcher.Form method="post">
              <input type="hidden" name="intent" value="project_workspace_clear" />
              <Button
                type="submit"
                variant="ghost"
                size="sm"
                disabled={submitting}
                data-testid="project-workspace-clear"
              >
                <Icon name="x" />
                消去
              </Button>
            </fetcher.Form>
          </CardBody>
        </Card>
      </section>

      {/* ADR-0043 D1（Phase 52 / G16）: 案件は「リポジトリ」を複数持つ（論文とコード、git ではない
          データの置き場）。`is_primary` の 1 件が上の「作業場所」と同じものを指す。並び・primary の
          付け替え・削除できるかどうかは celeris が決めるので、ここは表示と中継だけ。 */}
      <section aria-labelledby="project-repos-heading" data-testid="project-repos-section" className="space-y-4">
        <SectionTitle icon="database" id="project-repos-heading" count={repos.length}>
          リポジトリ
        </SectionTitle>
        <ProjectRepos projectId={project.id} repos={repos} clusters={clusters} />
      </section>

      {/* PR と取り込み（ADR-0043 D5、Phase 54 / G18）。タスク × リポジトリごとに最新の 1 件を
          celeris が新しい順で返すので、並べ替えも集計もしない。操作はタスクの「変更」で行う。 */}
      <section
        aria-labelledby="project-integrations-heading"
        data-testid="project-integrations-section"
        className="space-y-4"
      >
        <SectionTitle icon="gitBranch" id="project-integrations-heading" count={integrations.length}>
          PR と取り込み
        </SectionTitle>
        <ProjectIntegrations items={integrations} />
      </section>

      {/* celeris ADR-0079 D13 / D14（Phase R4b）: 案件は計画（途中目標の DAG）を持たなくなった。以前の途中目標は
          状態のまま凍結した履歴として読み取り専用で出す（作成・Go / 再設計・一時停止・判定の操作は出さない。
          API の廃止は R5a）。途中目標が 1 件も無い案件では節ごと出さない。 */}
      {milestones.length > 0 && (
        <section aria-labelledby="milestones-heading" data-testid="milestones-section" className="space-y-4">
          <SectionTitle icon="target" id="milestones-heading" count={milestones.length}>
            以前の途中目標（読み取り専用）
          </SectionTitle>
          <p className={hintClass} data-testid="milestones-readonly-note">
            途中目標は root task の段階で表すようになりました（ADR-0079）。ここにあるのは以前の記録です。
          </p>
          <ul className="space-y-2">
            {milestones
              .slice()
              .sort((a, b) => a.seq - b.seq)
              .map((m) => (
                <li key={m.id} data-testid="milestone-row" data-milestone-id={m.id}>
                  <Card>
                    <CardBody className="space-y-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-xs text-fg-subtle">#{m.seq}</span>
                        <span className="break-words font-medium">{m.title}</span>
                        <Badge
                          tone={milestoneStatusTone(m.status)}
                          data-testid="milestone-status"
                          data-status-badge="milestone"
                        >
                          {milestoneStatusLabel(m.status)}
                        </Badge>
                      </div>
                      {m.description && <p className="break-words text-sm text-fg-muted">{m.description}</p>}
                    </CardBody>
                  </Card>
                </li>
              ))}
          </ul>
        </section>
      )}

      {/* 「この方針で進める」（監査 H3、docs/celeris-api-v1.md §3.61）。押すと秘書に分解の仕事が 1 件立ち、
          仕事の木が増えていく。案件が「提案中」でも押せる（celeris が「進行中」にする）。 */}
      <section aria-labelledby="project-plan-heading" className="space-y-4">
        <SectionTitle icon="sparkles" id="project-plan-heading">
          この方針で進める
        </SectionTitle>
        <Card>
          <CardHeader
            icon="sparkles"
            title="分解を CoS に頼む"
            description="CoS が、依頼文・ここまでのやり取りとあなたの一言をまとめて、仕事に分解します。返事は待ちません（仕事の木が増えていきます）。"
          />
          <CardBody>
            <fetcher.Form method="post" data-testid="project-plan-form" className="space-y-3">
              <input type="hidden" name="intent" value="project_plan" />
              {/* celeris ADR-0079 D13（Phase R4b）: 「案件計画を提案させる」（`mode: milestones`）と途中目標の指定は
                  出さない（案件は計画を持たない。API の廃止は R5a）。進め方は従来どおりの分解だけ。 */}
              <input type="hidden" name="mode" value="decompose" />
              <div>
                <label htmlFor="project-plan-note" className={labelClass}>
                  ひとこと（任意）
                </label>
                <textarea
                  id="project-plan-note"
                  name="note"
                  rows={2}
                  data-testid="project-plan-note"
                  placeholder="例: 急がなくてよい。まず関連研究から。"
                  className={`${textareaClass} mt-1.5 w-full`}
                />
              </div>
              <Button type="submit" variant="primary" size="sm" disabled={submitting} data-testid="project-plan-submit">
                <Icon name="sparkles" />
                この方針で進める
              </Button>
            </fetcher.Form>
          </CardBody>
        </Card>
      </section>

      <section aria-labelledby="work-tree-heading" className="space-y-4">
        <SectionTitle icon="gitBranch" id="work-tree-heading" count={workTasks.length}>
          仕事の木
        </SectionTitle>
        {/* ADR-0044 D1（Phase 53）: 秘書に分解を頼むだけでなく、人が直接タスクを足せる。
            ボード（`/board?project=…`）でこの案件のタスクを並べて見られる。 */}
        <div className="flex flex-wrap items-center gap-3">
          <Link
            to={`/board?project=${encodeURIComponent(project.id)}`}
            data-testid="project-board-link"
            className={buttonClass({ variant: "secondary", size: "sm" })}
          >
            <Icon name="layers" />
            ボードで見る
          </Link>
        </div>
        <AddTaskForm projectId={project.id} org={org.items} testId="project-add-task" />
        {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
        <p className="text-sm text-fg-subtle lg:text-xs">
          パッと見て、おかしな方針を立てていないかを確かめるための図です。四角を押すと裏方のタスクへ移ります。
          対話の返事や報告のまとめといった裏方の作業は出しません。
        </p>
        {workTasks.length === 0 ? (
          <EmptyState icon="gitBranch" title="この案件のタスクはまだありません" />
        ) : (
          <>
            <WorkTree graph={graph} />
            {/* SPEC §3.4「おかしなことをしていたら、誰に言うかを決めてその担当に直接言う」。
                木のノード（タスク）の担当へ、この案件を選んだ状態で話しかける導線（Phase G13b-2）。
                Phase 31: draft には「Go」（accept）、failed/cancelled には「やり直す」（retry）も
                ここから直接できる（担当がいないタスクも拾えるよう、絞り込みは assignee 限定をやめた）。 */}
            <ul className="space-y-1" data-testid="work-tree-assignees">
              {workTasks
                .filter((t) => t.assignee || t.status === "draft" || t.status === "failed" || t.status === "cancelled")
                .map((t) => (
                  <WorkTreeTaskRow
                    key={t.id}
                    task={t}
                    projectId={project.id}
                    orgName={orgById.get(t.assignee ?? "")?.name}
                  />
                ))}
            </ul>
          </>
        )}
      </section>

      <section aria-labelledby="project-reports-heading" className="space-y-4">
        <SectionTitle icon="send" id="project-reports-heading" count={reports.items.length}>
          報告
        </SectionTitle>
        {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
        <p className="text-sm text-fg-subtle lg:text-xs">
          この案件について、各段から上がってきた報告です。全体の未読（CoS まで上がったもの）は
          <Link to="/reports" className={cn(touchLinkClass, "mx-1 underline underline-offset-2")}>
            報告
          </Link>
          の画面で流し見できます。
        </p>
        {reports.items.length === 0 ? (
          <EmptyState icon="send" title="この案件の報告はまだありません" />
        ) : (
          <ReportsList items={reports.items} projects={[project]} org={org.items} fetchedAt={fetchedAt} />
        )}
      </section>

      <section aria-labelledby="project-artifacts-heading" data-testid="artifacts-section" className="space-y-4">
        <SectionTitle icon="file" id="project-artifacts-heading" count={artifactRows.length}>
          成果物
        </SectionTitle>
        {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
        <p className="text-sm text-fg-subtle lg:text-xs">
          調査結果の文書とリンク集はここで読めます。コードは置き場所（普段のパス）へのリンクで示します。
        </p>
        {artifactRows.length === 0 ? (
          <EmptyState icon="file" title="この案件の成果物はまだありません" />
        ) : (
          <ArtifactsList rows={artifactRows} fetchedAt={fetchedAt} />
        )}
      </section>

      {/* ADR-0044 D7（Phase 57 / G20）: 文書は別のルート（`/projects/:id/docs`）。ここは入口だけ。 */}
      <section aria-labelledby="project-docs-heading" data-testid="project-docs-section" className="space-y-4">
        <SectionTitle icon="book" id="project-docs-heading">
          {DOCS_TAB_LABEL}
        </SectionTitle>
        <p className="text-sm text-fg-subtle lg:text-xs">{DOCS_SECTION_DESCRIPTION}</p>
        <Link
          to={`/projects/${project.id}/docs`}
          data-testid="project-docs-link"
          className={buttonClass({ variant: "secondary", size: "sm" })}
        >
          <Icon name="book" />
          {DOCS_TAB_LABEL}を開く
        </Link>
      </section>
    </div>
  );
}

/**
 * 案件のヘッダの「一時停止／再開」「中止（確認付き）」「アーカイブ／アーカイブ解除（確認付き）」
 * （ADR-0044 D6、docs/celeris-api-v1.md §3.84〜3.88。Phase 55 / G19）。
 *
 * 確認は `~/components/task-changes.tsx` の「捨てる（確認）」と同じ 2 段の fetcher フォーム
 * （`window.confirm` ではなく画面の中に出す。テストからも押せる）。
 * **押せるかどうかの最終判断は celeris**（409 `invalid_transition`）。ここは
 * `~/lib/lifecycle.ts::projectLifecycleButtons` で「その状態で意味のあるボタン」だけを出すだけで、
 * アーカイブは終端でなくても消さずに `disabled` にして理由を添える（何をすれば押せるかが分かるように）。
 */
function ProjectLifecycleActions({ project }: { project: ProjectDetail["project"] }) {
  const fetcher = useFetcher<ProjectOpOutcome>({ key: `project-lifecycle-${project.id}` });
  const busy = fetcher.state !== "idle";
  const [confirming, setConfirming] = useState<"cancel" | "archive" | null>(null);
  const buttons = projectLifecycleButtons(project);

  // フェーズ 71（ADR-0055 D2 ラウンド 3）: 一時停止／再開／中止はこのカードの主役の操作なので直接出す
  // （モバイルは縦積み・全幅、`sm:` から元どおりの横並び）。アーカイブ／アーカイブ解除は使う頻度が低い
  // 「その他の操作」として details に畳む（secondary actions in a details disclosure）。
  const hasOther = buttons.archive || buttons.unarchive;
  return (
    <div className="space-y-2" data-testid="project-lifecycle">
      <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center">
        {buttons.pause && (
          <fetcher.Form method="post">
            <input type="hidden" name="intent" value="project_pause" />
            <Button
              type="submit"
              variant="secondary"
              size="sm"
              disabled={busy}
              data-testid="project-pause"
              className="w-full sm:w-auto"
            >
              <Icon name="clock" />
              {PAUSE_LABEL}
            </Button>
          </fetcher.Form>
        )}
        {buttons.resume && (
          <fetcher.Form method="post">
            <input type="hidden" name="intent" value="project_resume" />
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={busy}
              data-testid="project-resume"
              className="w-full sm:w-auto"
            >
              <Icon name="play" />
              {RESUME_LABEL}
            </Button>
          </fetcher.Form>
        )}
        {buttons.cancel && (
          <Button
            type="button"
            variant="danger"
            size="sm"
            disabled={busy}
            onClick={() => setConfirming("cancel")}
            data-testid="project-cancel"
            className="w-full sm:w-auto"
          >
            <Icon name="ban" />
            {CANCEL_LABEL}
          </Button>
        )}
      </div>
      {hasOther && (
        <details data-testid="project-lifecycle-other">
          <summary
            className={cn(
              hintClass,
              "flex min-h-11 cursor-pointer items-center list-none underline underline-offset-2",
            )}
          >
            その他の操作（アーカイブ）
          </summary>
          <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center">
            {buttons.archive && (
              <Button
                type="button"
                variant="secondary"
                size="sm"
                disabled={busy || !buttons.archiveEnabled}
                title={buttons.archiveEnabled ? undefined : ARCHIVE_ONLY_TERMINAL_HINT}
                onClick={() => setConfirming("archive")}
                data-testid="project-archive"
                className="w-full sm:w-auto"
              >
                <Icon name="folder" />
                {ARCHIVE_LABEL}
              </Button>
            )}
            {buttons.unarchive && (
              <fetcher.Form method="post">
                <input type="hidden" name="intent" value="project_unarchive" />
                <Button
                  type="submit"
                  variant="secondary"
                  size="sm"
                  disabled={busy}
                  data-testid="project-unarchive"
                  className="w-full sm:w-auto"
                >
                  <Icon name="rotate" />
                  {UNARCHIVE_LABEL}
                </Button>
              </fetcher.Form>
            )}
          </div>
          {buttons.archive && !buttons.archiveEnabled && (
            <p className={cn(hintClass, "mt-1")} data-testid="project-archive-hint">
              {ARCHIVE_ONLY_TERMINAL_HINT}
            </p>
          )}
        </details>
      )}
      {confirming === "cancel" && (
        <Alert tone="danger" data-testid="project-cancel-confirm">
          <p>{projectCancelConfirmText(project.title)}</p>
          <fetcher.Form method="post" className="flex flex-wrap items-center gap-2 pt-1">
            <input type="hidden" name="intent" value="project_cancel" />
            <Button type="submit" variant="danger" size="sm" disabled={busy} data-testid="project-cancel-submit">
              <Icon name="ban" />
              {CANCEL_CONFIRM_LABEL}
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => setConfirming(null)}>
              {CANCEL_STOP_LABEL}
            </Button>
          </fetcher.Form>
        </Alert>
      )}
      {confirming === "archive" && (
        <Alert tone="info" data-testid="project-archive-confirm">
          <p>{projectArchiveConfirmText(project.title)}</p>
          <fetcher.Form method="post" className="flex flex-wrap items-center gap-2 pt-1">
            <input type="hidden" name="intent" value="project_archive" />
            <Button type="submit" variant="secondary" size="sm" disabled={busy} data-testid="project-archive-submit">
              <Icon name="folder" />
              {ARCHIVE_CONFIRM_LABEL}
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => setConfirming(null)}>
              {CANCEL_STOP_LABEL}
            </Button>
          </fetcher.Form>
        </Alert>
      )}
      <ProjectActionFlash outcome={fetcher.data} />
    </div>
  );
}

/**
 * 「タスクを追加」（ADR-0044 D1、Phase 53）。案件のヘッダと途中目標のカードの両方から同じ形で開く
 * （`milestoneId` を渡すとその途中目標に属するタスクになる）。
 *
 * **人が作ったタスクは待機中（ready）で始まる**（`POST /tasks` の既定。人は Go を出す側なので
 * draft を挟まない）。検証は celeris（題名・目的・受け入れ条件が空なら 422）に任せ、その文言をそのまま出す。
 */
function AddTaskForm({
  projectId,
  milestoneId,
  org,
  testId,
}: {
  projectId: string;
  milestoneId?: string;
  org: OrgNode[];
  testId: string;
}) {
  const fetcher = useFetcher<ProjectOpOutcome>({ key: `task-create-${milestoneId ?? projectId}` });
  const busy = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : undefined;

  return (
    <details data-testid={testId} className="rounded-lg border border-border bg-surface-2/40 p-3">
      <summary className="cursor-pointer select-none text-sm font-medium text-fg">タスクを追加</summary>
      <div className="mt-3 space-y-3">
        <ProjectActionFlash outcome={fetcher.data} />
        <fetcher.Form method="post" className="space-y-3" data-testid={`${testId}-form`}>
          <input type="hidden" name="intent" value="task_create" />
          {milestoneId && <input type="hidden" name="milestone_id" value={milestoneId} />}
          <div>
            <label htmlFor={`${testId}-title`} className={labelClass}>
              題名
            </label>
            <input
              id={`${testId}-title`}
              name="title"
              type="text"
              data-testid={`${testId}-title`}
              className={`${inputClass} mt-1.5 w-full`}
            />
            <FieldErrors error={error} field="title" />
          </div>
          <div>
            <label htmlFor={`${testId}-objective`} className={labelClass}>
              目的
            </label>
            <textarea
              id={`${testId}-objective`}
              name="objective"
              rows={3}
              data-testid={`${testId}-objective`}
              className={`${textareaClass} mt-1.5 w-full`}
            />
            <FieldErrors error={error} field="objective" />
          </div>
          <div>
            <label htmlFor={`${testId}-acceptance`} className={labelClass}>
              終わったと言える条件
            </label>
            <input
              id={`${testId}-acceptance`}
              name="acceptance"
              type="text"
              placeholder="例: 候補が 3 件以上まとまっている"
              data-testid={`${testId}-acceptance`}
              className={`${inputClass} mt-1.5 w-full`}
            />
            <FieldErrors error={error} field="acceptance" />
          </div>
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <label htmlFor={`${testId}-assignee`} className={labelClass}>
                担当
              </label>
              <select
                id={`${testId}-assignee`}
                name="assignee"
                defaultValue=""
                data-testid={`${testId}-assignee`}
                className={`${selectClass} mt-1.5`}
              >
                <option value="">（celeris に任せる）</option>
                {org.map((node) => (
                  <option key={node.id} value={node.id}>
                    {node.name}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor={`${testId}-tier`} className={labelClass}>
                レベル
              </label>
              <select
                id={`${testId}-tier`}
                name="tier"
                defaultValue=""
                data-testid={`${testId}-tier`}
                className={`${selectClass} mt-1.5`}
              >
                <option value="">（既定）</option>
                {TIERS.map((t) => (
                  <option key={t} value={t}>
                    {tierLabel(t)}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor={`${testId}-priority`} className={labelClass}>
                優先度
              </label>
              <select
                id={`${testId}-priority`}
                name="priority"
                defaultValue="P2"
                data-testid={`${testId}-priority`}
                className={`${selectClass} mt-1.5`}
              >
                {PRIORITY_LABELS.map((p) => (
                  <option key={p} value={p}>
                    {priorityFullLabel(p)}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor={`${testId}-category`} className={labelClass}>
                種類
              </label>
              <select
                id={`${testId}-category`}
                name="category"
                defaultValue="other"
                data-testid={`${testId}-category`}
                className={`${selectClass} mt-1.5`}
              >
                {TASK_CATEGORIES.map((c) => (
                  <option key={c} value={c}>
                    {taskCategoryLabel(c)}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <Button type="submit" variant="primary" size="sm" disabled={busy} data-testid={`${testId}-submit`}>
            <Icon name="plus" />
            追加（待機中で始まります）
          </Button>
        </fetcher.Form>
      </div>
    </details>
  );
}

/**
 * 仕事の木の 1 行（Phase 31。実機の事故、2026-09-18）。`draft` には「Go」（`/tasks/:id/approve` の
 * `Trigger::Accept`）、`failed`/`cancelled` には「やり直す」（`/tasks/:id/retry`）を直接置く。
 * どちらも `/tasks/:id` の action へ直接 POST する fetcher（この画面の action は project 単位の
 * intent しか扱わないため）。成功後の遷移は「やり直す」だけ（新しいタスクが増えるので、そちらを見せる）。
 */
function WorkTreeTaskRow({
  task,
  projectId,
  orgName,
}: {
  task: ProjectTaskView;
  projectId: string;
  orgName: string | undefined;
}) {
  const goFetcher = useFetcher<TransitionOutcome>({ key: `work-tree-go-${task.id}` });
  const going = goFetcher.state !== "idle";
  const retryFetcher = useFetcher<RetryOutcome>({ key: `work-tree-retry-${task.id}` });
  const retrying = retryFetcher.state !== "idle";
  const navigate = useNavigate();
  useEffect(() => {
    if (retryFetcher.data?.ok) {
      navigate(`/tasks/${retryFetcher.data.result.task_id}`);
    }
  }, [retryFetcher.data, navigate]);

  return (
    <li
      className="flex flex-wrap items-center gap-2 text-sm"
      data-testid="work-tree-task-row"
      data-task-status={task.status}
    >
      <Badge tone="neutral">{taskStatusLabel(task.status)}</Badge>
      <Link to={`/tasks/${task.id}`} className={cn(touchLinkClass, "underline underline-offset-2")}>
        {task.title}
      </Link>
      {task.assignee && (
        <>
          {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
          <span className="text-sm text-fg-subtle lg:text-xs">担当: {orgName ?? task.assignee}</span>
          <Link
            to={`/org/${encodeURIComponent(task.assignee)}?project=${encodeURIComponent(projectId)}`}
            data-testid="work-tree-talk"
            data-assignee={task.assignee}
            className={buttonClass({ variant: "ghost", size: "xs" })}
          >
            <Icon name="message" />
            担当に話す
          </Link>
        </>
      )}
      {task.status === "draft" && (
        <goFetcher.Form method="post" action={`/tasks/${task.id}`}>
          <input type="hidden" name="intent" value="approve" />
          <input type="hidden" name="expected_status" value="draft" />
          <Button type="submit" variant="success" size="xs" disabled={going} data-testid="work-tree-go">
            <Icon name="check" />
            Go
          </Button>
        </goFetcher.Form>
      )}
      {(task.status === "failed" || task.status === "cancelled") && (
        <retryFetcher.Form method="post" action={`/tasks/${task.id}`} className="flex items-center gap-2">
          <input type="hidden" name="intent" value="retry" />
          {/* ADR-0055 D1-2/D1-4: タップ領域 44 以上、モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。
              ADR-0070 D2 追記（Phase 116）: 既定は ready。draft のまま始めたいときだけチェックする。 */}
          <label className="flex min-h-11 items-center gap-1 text-sm text-fg-subtle lg:text-xs">
            <input type="checkbox" name="draft" value="true" className={checkboxClass} />
            draft のまま始める
          </label>
          <Button type="submit" variant="primary" size="xs" disabled={retrying} data-testid="work-tree-retry">
            <Icon name="rotate" />
            やり直す
          </Button>
        </retryFetcher.Form>
      )}
      {goFetcher.data && !goFetcher.data.ok && <ErrorFlash error={goFetcher.data.error} />}
      <RetryFlash outcome={retryFetcher.data} />
    </li>
  );
}

export function ErrorBoundary({ error }: Route.ErrorBoundaryProps) {
  if (isRouteErrorResponse(error) && error.data && typeof error.data === "object" && "kind" in error.data) {
    const data = error.data as CelerisRouteErrorData;
    if (data.kind === "unavailable") {
      return (
        <main className="p-4">
          <CelerisBanner celerisApiUrl={data.baseUrl ?? ""} problem={null} />
          <RouteRecovery />
        </main>
      );
    }
    return (
      <main className="p-4">
        <h1 className="text-xl font-semibold">
          {data.status === 404 ? "案件が見つかりません" : `エラー ${data.status}`}
        </h1>
        <p className="mt-2 text-sm text-fg-muted">{data.detail}</p>
        {isTransientStatus(data.status) && <RouteRecovery />}
      </main>
    );
  }
  return (
    <main className="p-4">
      <h1 className="text-xl font-semibold">エラー</h1>
      <p className="mt-2 text-sm text-fg-muted">予期しないエラーが起きました。</p>
      <RouteRecovery />
    </main>
  );
}

/**
 * celeris ADR-0072「Phase F6 実装時の決定」: 案件の名前・説明（依頼文）・slug を後から変える（`PATCH /projects/{id}`）。
 * 折り畳みで出し、送った値の検証（空・長さ・slug の綴りと重複）は celeris が行う（422 / 409 の文言をそのまま出す）。
 * 説明を変えても CoS への再依頼にはならない（次に分解・案件計画を起こしたときに今の文面が渡る）。
 */
function ProjectEditForm({ project }: { project: Project }) {
  const fetcher = useFetcher<ProjectOpOutcome>({ key: `project-edit-${project.id}` });
  const submitting = fetcher.state !== "idle";
  const error = fetcher.data && !fetcher.data.ok ? fetcher.data.error : null;
  return (
    <details className="rounded-lg border border-border p-3" data-testid="project-edit">
      <summary className="cursor-pointer select-none text-sm font-medium text-fg">名前・説明を編集</summary>
      <fetcher.Form method="post" className="mt-3 space-y-3" data-testid="project-edit-form">
        <input type="hidden" name="intent" value="project_edit" />
        <input type="hidden" name="slug_current" value={project.slug ?? ""} />
        <div>
          <label htmlFor="project-edit-title" className={labelClass}>
            名前
          </label>
          <input
            id="project-edit-title"
            name="title"
            type="text"
            required
            maxLength={200}
            defaultValue={project.title}
            data-testid="project-edit-title"
            className={`${inputClass} mt-1.5 w-full`}
          />
          <FieldErrors error={error} field="title" />
        </div>
        <div>
          <label htmlFor="project-edit-request" className={labelClass}>
            説明（依頼文）
          </label>
          <textarea
            id="project-edit-request"
            name="request"
            rows={6}
            required
            maxLength={20000}
            defaultValue={project.request}
            data-testid="project-edit-request"
            className={`${textareaClass} mt-1.5 w-full`}
          />
          <p className={hintClass}>
            変えても CoS への再依頼にはなりません。次に分解・案件計画を起こしたときに今の文面が渡ります。
          </p>
          <FieldErrors error={error} field="request" />
        </div>
        <div>
          <label htmlFor="project-edit-slug" className={labelClass}>
            知識ベースの置き場（slug）
          </label>
          <input
            id="project-edit-slug"
            name="slug"
            type="text"
            defaultValue={project.slug ?? ""}
            data-testid="project-edit-slug"
            className={`${inputClass} mt-1.5 w-full max-w-md font-mono`}
          />
          <p className={hintClass}>
            小文字の英数字とハイフン。変えても知識ベースのディレクトリ（projects/&lt;slug&gt;/）は動きません。
          </p>
          <FieldErrors error={error} field="slug" />
        </div>
        {error && !error.fields.title && !error.fields.request && !error.fields.slug && <ErrorFlash error={error} />}
        {fetcher.data?.ok && fetcher.data.op === "project_edit" && (
          <p className="text-sm text-fg-muted" data-testid="project-edit-saved">
            保存しました。
          </p>
        )}
        <Button type="submit" variant="primary" size="sm" disabled={submitting} data-testid="project-edit-submit">
          保存する
        </Button>
      </fetcher.Form>
    </details>
  );
}
