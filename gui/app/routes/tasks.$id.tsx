import { lazy, Suspense, useEffect } from "react";
import { isRouteErrorResponse, Link, useFetcher, useNavigate, useNavigation, useSearchParams } from "react-router";
import { browserOwnerView } from "~/browser-owner.server";
import type { RetryOutcome, TaskCommentOutcome, TaskRereviewOutcome, TransitionOutcome } from "~/celeris/action-types";
import { liveViewRelayAvailable } from "~/celeris/browser-live.server";
import { getCelerisClient } from "~/celeris/client.server";
import { type CelerisRouteErrorData, celerisErrorResponse } from "~/celeris/errors";
import { loadTaskDetail, runTaskDetailAction, type TaskDetailData } from "~/celeris/task-detail.server";
import type { TaskDetail, TimelineItem } from "~/celeris/types";
import { ClusterJobWaitBanner } from "~/components/ClusterJobWaitBanner";
import { HelpLink } from "~/components/HelpLink";
import { IntegrationRepairPanel } from "~/components/IntegrationRepairPanel";
import { RouteRecovery } from "~/components/RouteRecovery";
import { TaskRoutingPanel } from "~/components/TaskRoutingPanel";
import { ArtifactRow } from "~/components/task-detail/ArtifactRow";
import { FailureBanner } from "~/components/task-detail/FailureBanner";
import { OverviewTab } from "~/components/task-detail/OverviewTab";
import { TaskTabSkeleton } from "~/components/task-detail/TaskTabSkeleton";
import { TaskTabs } from "~/components/task-detail/TaskTabs";
import { TimelineTab } from "~/components/task-detail/TimelineTab";
import { Badge, GenreLabel, KindBadge, RoleLabel, StatusBadge } from "~/components/ui/badge";
import { buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { touchLinkClass } from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { Alert, EmptyState } from "~/components/ui/misc";
import {
  ASSIGNED_WHY_LABEL,
  assignedScoreLabel,
  parseTaskTab,
  priorityFullLabel,
  taskCategoryLabel,
} from "~/lib/labels";
import { isLiveStatusScreen } from "~/lib/live-status";
import { isTransientStatus } from "~/lib/recovery";
import { revalidateAfterActionErrors } from "~/lib/revalidate";
import { EXECUTION_PHASE_LABEL, EXECUTION_PHASE_TONE } from "~/lib/task-execution";
import { cn } from "~/lib/utils";
import { CelerisBanner } from "~/root";
import type { Route } from "./+types/tasks.$id";

const BrowserWaitsPanel = lazy(() =>
  import("~/components/BrowserWaitsPanel").then((m) => ({ default: m.BrowserWaitsPanel })),
);

// Phase 77（ADR-0055 性能予算）: 「変更」「ファイル」タブの本体（`~/components/task-changes.tsx`・
// `~/components/task-files.tsx`）は、5 つあるタブのうち一度に 1 つしか出ない（`?tab=` で切り替え）のに
// これまで両方とも静的 import していたので、どのタブを開いても他の 4 タブぶんの JS まで初回に届いていた。
// `React.lazy` にして、実際に選んだタブのチャンクだけを取りに行くようにする（ADR-0043 D5/D6 の中身・
// `~/routes/tasks.$id.changes.tsx`・`~/routes/tasks.$id.files.tsx` という兄弟ルートからの静的 import は
// そのまま残すので、そちらの動作・バンドルは変えない）。
const TaskChanges = lazy(() => import("~/components/task-changes").then((m) => ({ default: m.TaskChanges })));

const TaskFiles = lazy(() => import("~/components/task-files").then((m) => ({ default: m.TaskFiles })));

// celeris ADR-0079 D14（Phase R4b、ADR-0055 性能予算）: 「木」タブの本体・止まっている理由の帯は、
// 開いたとき・止まっているときだけ要るので初回の JS に載せない（task 系ルートの初回 JS は予算まで 3KB しか無い）。
const TaskTreeTab = lazy(() => import("~/components/TaskTreeTab").then((m) => ({ default: m.TaskTreeTab })));

const TaskHoldBanner = lazy(() => import("~/components/TaskHoldBanner").then((m) => ({ default: m.TaskHoldBanner })));

export function meta(_: Route.MetaArgs) {
  return [{ title: "タスク詳細 - Celeris" }];
}

// 409 / 422 の action 後も再検証する（docs/adr/0005 D2）。
export const shouldRevalidate = revalidateAfterActionErrors;

export async function loader({ params, request }: Route.LoaderArgs): Promise<TaskDetailData> {
  try {
    return await loadTaskDetail(
      getCelerisClient(),
      params.id,
      request,
      await browserOwnerView(request),
      liveViewRelayAvailable(),
    );
  } catch (e) {
    throw celerisErrorResponse(e);
  }
}

export async function action({ request, params }: Route.ActionArgs) {
  const form = await request.formData();
  return runTaskDetailAction(getCelerisClient(), params.id, form, request.signal);
}

export default function TaskDetailPage({ loaderData }: Route.ComponentProps) {
  const {
    detail,
    browserRuns,
    browserWaits,
    browserOwner,
    liveViews,
    events,
    artifacts,
    timeline,
    comments,
    fetchedAt,
    org,
    milestones,
    genres,
    assignedEvent,
    routing,
    files,
    changes,
    taskTree,
    humanReview,
    place,
  } = loaderData;
  const { task } = detail;
  const [searchParams] = useSearchParams();
  const tab = parseTaskTab(searchParams.get("tab"));
  // Phase 77（ADR-0055 D3「体感速度」）: タブを切り替えると `?tab=` が変わって loader が再実行される
  // （タブごとに `changes`/`files`/`timeline` の中身が違うため）。その間は前のタブの中身がそのまま
  // 残るだけで何も動いて見えないので、この画面への遷移が pending の間はタブの中身をスケルトンに差し替える
  // （高さは `TaskTabSkeleton` の概算。レイアウトのガタつきを避けるため中身の種類では変えない）。
  const navigation = useNavigation();
  const tabNavigationPending = navigation.state === "loading" && navigation.location?.pathname === `/tasks/${task.id}`;
  // 操作の結果は fetcher に載せる（監査 H1。SSE の再検証で `actionData` が消えるのを避ける）。
  const fetcher = useFetcher<TransitionOutcome>();
  const submitting = fetcher.state !== "idle";
  // Phase 31: 「やり直す」は別のタスクを新しく作る（`TransitionOutcome` とは形が違う）ので別の fetcher。
  // 成功したら新しいタスクへ遷移する（fetcher はナビゲーションを行わないので `useNavigate` で明示的に行う）。
  const retryFetcher = useFetcher<RetryOutcome>();
  const retrying = retryFetcher.state !== "idle";
  // ADR-0070 D2（Phase 116）: 失敗バナーの「再レビュー」「取り下げ」。retry は上の retryFetcher を共有する。
  const rereviewFetcher = useFetcher<TaskRereviewOutcome>({ key: `task-rereview-${task.id}` });
  const rereviewing = rereviewFetcher.state !== "idle";
  const dismissFetcher = useFetcher<TaskCommentOutcome>({ key: `task-dismiss-${task.id}` });
  const dismissing = dismissFetcher.state !== "idle";
  const navigate = useNavigate();
  useEffect(() => {
    if (retryFetcher.data?.ok) {
      navigate(`/tasks/${retryFetcher.data.result.task_id}`);
    }
  }, [retryFetcher.data, navigate]);

  return (
    <div className="space-y-8">
      {/* ヒーロー: タイトル・status/kind/role・ID・クラスタ / 親・操作(DAG) */}
      <section aria-labelledby="header-heading" data-testid="header-section">
        <div className="rounded-2xl border border-border bg-surface p-6 shadow-sm">
          <div className="flex flex-wrap items-start justify-between gap-5">
            <div className="min-w-0 flex-1 space-y-2.5">
              <div className="flex flex-wrap items-center gap-2">
                {/* U-G32-2 の解消（Phase 84）: 詳細は開いたまま SSE の再検証で更新され続ける画面なので、
                    状態バッジをライブリージョンにする（`~/lib/live-status.ts`）。 */}
                <StatusBadge
                  status={task.status}
                  role={isLiveStatusScreen("task-detail") ? "status" : undefined}
                  data-testid="task-status"
                />
                {/* celeris ADR-0072 D20（Phase E5）: 今どの段階か（計画中/実行中/修復中/検証中）。
                    計画の無いタスク・終端のタスクには無い（`detail.execution?.phase` が null）。 */}
                {detail.execution?.phase && (
                  <Badge tone={EXECUTION_PHASE_TONE[detail.execution.phase]} data-testid="task-execution-phase">
                    {EXECUTION_PHASE_LABEL[detail.execution.phase]}
                  </Badge>
                )}
                <KindBadge kind={task.kind} data-testid="task-kind" />
                <RoleLabel role={detail.role ?? "-"} data-testid="task-role" />
                {/* 分野（ADR-0027 D1）。role と同じ理由で色分けはせずテキストのラベルだけ。分野なしは "-"。 */}
                <GenreLabel genre={detail.genre ?? "-"} data-testid="task-genre" />
                {/* ADR-0044 D3（Phase 53）: 優先度・種類・ラベルは一目で分かるところに置く。 */}
                <Badge tone="primary" data-testid="task-priority">
                  {priorityFullLabel(detail.priority_label)}
                </Badge>
                <Badge tone="neutral" data-testid="task-category">
                  {taskCategoryLabel(task.category ?? "other")}
                </Badge>
                {(task.labels ?? []).map((label) => (
                  <Badge key={label} tone="teal" data-testid="task-label">
                    {label}
                  </Badge>
                ))}
              </div>
              <h1 id="header-heading" className="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span data-testid="task-id" className="font-mono text-xs text-fg-subtle">
                  {task.id}
                </span>
                <HelpLink anchor="screens" label="画面ごとの説明" />
              </h1>
              <p className="break-words text-2xl font-bold tracking-tight text-fg" data-testid="task-title">
                {task.title}
              </p>
              {/* 裏方から戻れる導線（監査 M2）: 案件・担当・途中目標。 */}
              <div
                className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-fg-muted"
                data-testid="task-place"
              >
                {place.projectId && (
                  <p>
                    案件:{" "}
                    <Link
                      to={`/projects/${place.projectId}`}
                      data-testid="task-project-link"
                      className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                    >
                      {place.projectTitle ?? place.projectId}
                    </Link>
                  </p>
                )}
                {place.assigneeId && (
                  <p>
                    担当:{" "}
                    <Link
                      to={
                        place.assigneeId === "secretary"
                          ? "/org/secretary"
                          : `/org/${encodeURIComponent(place.assigneeId)}`
                      }
                      data-testid="task-assignee-link"
                      className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                    >
                      {place.assigneeName ?? place.assigneeId}
                    </Link>
                    {/* ADR-0046 D5（Phase 59 / G21）: 「なぜこの担当か」。matching が決めたタスクにだけ出る
                        （`Event::Assigned`。明示の assignee で作られたタスクには無い）。 */}
                    {assignedEvent && assignedEvent.node === place.assigneeId && (
                      <span
                        className="ml-1.5 text-sm text-fg-subtle lg:text-xs"
                        data-testid="task-assigned-why"
                        title={`${ASSIGNED_WHY_LABEL}: ${assignedEvent.reason}（${assignedScoreLabel(assignedEvent.score)}）`}
                      >
                        （{ASSIGNED_WHY_LABEL}: {assignedEvent.reason}）
                      </span>
                    )}
                  </p>
                )}
                {place.milestoneTitle && <p data-testid="task-milestone">途中目標: {place.milestoneTitle}</p>}
              </div>
              {/* celeris ADR-0069 D5: なぜこの担当・harness・lane・model か。閉じた状態は 1 行。 */}
              <TaskRoutingPanel view={routing} />
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-fg-muted">
                {detail.cluster && (
                  <p data-testid="task-cluster">
                    cluster:{" "}
                    <Link to="/clusters" className={cn(touchLinkClass, "font-medium text-primary hover:underline")}>
                      {detail.cluster}
                    </Link>
                    {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
                    <span className="ml-2 text-sm text-fg-subtle lg:text-xs" data-testid="task-workspace-note">
                      {/* ADR-0039 D3（Phase G13k）: 編集は手元の作業ディレクトリで、検証はリモートで。 */}
                      {detail.workspace_dir
                        ? `手元の写し: ${detail.workspace_dir}（クラスタ側の元のパスは表示されません）`
                        : "workspace_dir はクラスタ側ではなく手元の写しです（クラスタ側の元のパスは表示されません）。"}
                    </span>
                  </p>
                )}
                {task.parent_id && (
                  <p data-testid="task-parent">
                    親:{" "}
                    <Link
                      to={`/tasks/${task.parent_id}`}
                      className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                    >
                      {task.parent_id}
                    </Link>
                  </p>
                )}
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {/* 作業ツリー（ADR-0043 D6、Phase 52 / G16）。ADR-0044 D5 のタブの殻ができたので
                  「ファイル」タブに載せ替えた。兄弟のルート `/tasks/:id/files` も従来どおり動く
                  （同じ `~/components/task-files.tsx` を全画面で出す）。 */}
              <Link
                to={`/tasks/${task.id}?tab=files`}
                data-testid="task-files-link"
                className={buttonClass({ variant: "secondary", size: "sm" })}
              >
                <Icon name="folder" />
                ファイル
              </Link>
              {/* 変更の取り込み（ADR-0043 D5、Phase 54 / G18）。ADR-0044 D5 のタブの殻ができたので
                  「変更」タブに載せ替えた。兄弟のルート `/tasks/:id/changes` も従来どおり動く
                  （同じ `~/components/task-changes.tsx` を全画面で出す）。 */}
              <Link
                to={`/tasks/${task.id}?tab=changes`}
                data-testid="task-changes-link"
                className={buttonClass({ variant: "secondary", size: "sm" })}
              >
                <Icon name="gitBranch" />
                変更
              </Link>
              <Link
                to={`/graph?root=${task.id}`}
                data-testid="task-graph-link"
                className={buttonClass({ variant: "secondary", size: "sm" })}
              >
                <Icon name="gitBranch" />
                DAG で見る
              </Link>
            </div>
          </div>
        </div>
      </section>

      {/* celeris ADR-0120 D5: target drift に伴う integration repair。実装失敗の FailureBanner
          より上に、別の欄・色・ラベルで表示する。null / 欠落なら何も出さない。 */}
      <IntegrationRepairPanel repair={detail.integration_repair} />

      {/* ADR-0070 D1/D2（Phase 116）: failed のタスクは、原因の分類と「やり直す」「再レビュー」
          「取り下げ」をどのタブからでも見える位置に出す（受け入れ条件 D6(e)）。 */}
      {detail.failure && (
        <FailureBanner
          taskId={task.id}
          failure={detail.failure}
          actions={detail.actions}
          retryFetcher={retryFetcher}
          retrying={retrying}
          rereviewFetcher={rereviewFetcher}
          rereviewing={rereviewing}
          dismissFetcher={dismissFetcher}
          dismissing={dismissing}
        />
      )}

      {/* celeris ADR-0090 D5（Phase R7-1）: クラスタ job の durable wait（daemon が poll し、終われば続きの run）。 */}
      {detail.cluster_job_wait && <ClusterJobWaitBanner wait={detail.cluster_job_wait} />}

      {/* celeris ADR-0079 D10 / D14（Phase R4b）: 止まっている理由（理由なし・決定・基盤・承認）。
          候補のときだけ lazy の帯を読み、判定と文言は帯の側（`~/lib/tree.ts`）で行う。 */}
      {mayBeHeld(task.status, detail.execution, timeline.items) && (
        <Suspense fallback={null}>
          <TaskHoldBanner
            taskId={task.id}
            status={task.status}
            execution={detail.execution}
            timeline={timeline.items}
          />
        </Suspense>
      )}

      {/* ADR-0044 D5: 概要 / 木 / タイムライン / 変更 / ファイル / 成果物。`?tab=` が状態なのでリンクできる。 */}
      <TaskTabs
        current={tab}
        searchParams={searchParams}
        counts={{ timeline: timeline.items.length, artifacts: artifacts.items.length }}
      />

      {browserWaits.length > 0 && (
        <Suspense fallback={null}>
          <BrowserWaitsPanel waits={browserWaits} owner={browserOwner} />
        </Suspense>
      )}

      {tabNavigationPending ? (
        <TaskTabSkeleton />
      ) : (
        <>
          {tab === "overview" && (
            <OverviewTab
              browserRuns={browserRuns}
              liveViews={liveViews}
              detail={detail}
              artifactCount={artifacts.items.length}
              org={org}
              milestones={milestones}
              genres={genres}
              fetcher={fetcher}
              submitting={submitting}
              retryFetcher={retryFetcher}
              retrying={retrying}
              fetchedAt={fetchedAt}
              humanReview={humanReview}
              browserOwner={browserOwner}
            />
          )}

          {tab === "tree" && (
            <section aria-label="木" data-testid="tree-section" className="space-y-4">
              <Suspense fallback={<TaskTabSkeleton />}>
                <TaskTreeTab
                  view={taskTree?.data ?? null}
                  error={taskTree?.error ?? null}
                  currentId={task.id}
                  tree={task.tree}
                />
              </Suspense>
            </section>
          )}

          {tab === "timeline" && (
            <TimelineTab
              taskId={task.id}
              detail={detail}
              timeline={timeline}
              comments={comments}
              events={events}
              fetchedAt={fetchedAt}
            />
          )}

          {tab === "changes" && (
            <section aria-labelledby="changes-heading" data-testid="changes-section" className="space-y-4">
              <h2 id="changes-heading" className="text-[0.95rem] font-semibold text-fg">
                変更
              </h2>
              {/* ADR-0043 D5（Phase 54 / G18）の本物。中身は `~/components/task-changes.tsx`。
                  取り込みの `fetcher` と差分のリンクは兄弟のルート `/tasks/:id/changes` に出る
                  （「ファイル」タブと同じ作り）。Phase 77: `React.lazy`（このファイル冒頭）なので `Suspense` で包む。 */}
              {changes?.data ? (
                <Suspense fallback={<TaskTabSkeleton />}>
                  <TaskChanges
                    taskId={task.id}
                    changes={changes.data.changes}
                    diff={changes.data.diff}
                    diffError={changes.data.diffError}
                    diffRepo={changes.data.diffRepo}
                    diffPath={changes.data.diffPath}
                  />
                </Suspense>
              ) : changes?.error ? (
                <EmptyState icon="gitBranch" title="取り込める変更がありません" data-testid="task-changes-unavailable">
                  {changes.error.detail}
                </EmptyState>
              ) : null}
            </section>
          )}

          {tab === "files" && (
            <section aria-labelledby="files-heading" data-testid="files-section" className="space-y-4">
              <h2 id="files-heading" className="text-[0.95rem] font-semibold text-fg">
                ファイル
              </h2>
              {/* ADR-0043 D6（Phase 52 / G16）の本物。中身は `~/components/task-files.tsx`。
                  Phase 77: `React.lazy`（このファイル冒頭）なので `Suspense` で包む。 */}
              {files?.data ? (
                <Suspense fallback={<TaskTabSkeleton />}>
                  <TaskFiles
                    taskId={task.id}
                    tree={files.data.tree}
                    file={files.data.file}
                    fileError={files.data.fileError}
                    filePath={files.data.filePath}
                  />
                </Suspense>
              ) : files?.error ? (
                <EmptyState icon="folder" title="作業ツリーがありません" data-testid="task-files-unavailable">
                  {files.error.detail}
                </EmptyState>
              ) : null}
            </section>
          )}

          {tab === "artifacts" && (
            <section aria-labelledby="artifacts-heading" data-testid="artifacts-section">
              <Card>
                <CardHeader
                  icon="folder"
                  title={
                    <h2 id="artifacts-heading" className="text-[0.95rem] font-semibold text-fg">
                      成果物
                    </h2>
                  }
                />
                <CardBody>
                  {artifacts.items.length === 0 ? (
                    <EmptyState icon="folder" title="ありません。" />
                  ) : (
                    <ul className="space-y-3">
                      {artifacts.items.map((artifact) => (
                        <ArtifactRow
                          key={artifact.idx}
                          taskId={task.id}
                          artifact={artifact}
                          projectId={task.project_id ?? null}
                          taskTitle={task.title}
                          category={task.category ?? null}
                        />
                      ))}
                    </ul>
                  )}
                </CardBody>
              </Card>
            </section>
          )}
        </>
      )}
    </div>
  );
}

/**
 * celeris ADR-0079 D10（Phase R4b）: 止まっている理由の帯を読むかどうかの安い下見（終端でなく、承認待ち・
 * blocked の unit・`stall_detected` のどれかがある）。本当に出すかと文言は lazy の `TaskHoldBanner` が決める。
 */
function mayBeHeld(status: TaskDetail["task"]["status"], execution: TaskDetail["execution"], items: TimelineItem[]) {
  if (status === "done" || status === "failed" || status === "cancelled") return false;
  return (
    execution?.plan_approval != null ||
    (execution?.plan?.work_units ?? []).some((u) => u.status === "blocked") ||
    items.some((i) => i.kind === "event" && i.event.type === "stall_detected")
  );
}

/**
 * loader が `celerisErrorResponse` で投げた `Response` を `isRouteErrorResponse` で判別する
 * （docs/adr/0004-g1-decisions.md D6）。celeris 停止中はこのルート自身が root と同じバナーを出し
 * （200 にはならないが 500 でもない。§6.5「500 にしない」）、404 は「タスクが見つかりません」にする。
 */
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
      <main className="mx-auto max-w-2xl space-y-3 p-6">
        <h1 className="text-xl font-semibold text-fg">
          {data.status === 404 ? "タスクが見つかりません" : `エラー ${data.status}`}
        </h1>
        <Alert tone="danger">{data.detail}</Alert>
        {isTransientStatus(data.status) && <RouteRecovery />}
      </main>
    );
  }

  return (
    <main className="mx-auto max-w-2xl space-y-3 p-6">
      <h1 className="text-xl font-semibold text-fg">エラー</h1>
      <Alert tone="danger">予期しないエラーが起きました。</Alert>
      <RouteRecovery />
    </main>
  );
}
