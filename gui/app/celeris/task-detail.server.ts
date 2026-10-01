// `/tasks/:id`（タスク詳細）の loader / action の本体。ルート（`~/routes/tasks.$id.tsx`）は
// これを呼ぶ薄い層と画面だけを持ち、タブ・節の部品は `~/components/task-detail/` にある。
import { data } from "react-router";
import type { ActionError } from "~/celeris/action-types";
import { retryData, transitionData } from "~/celeris/actions.server";
import { loadBrowserRuns, loadTaskBrowserWaits } from "~/celeris/browser";
import type { CelerisClient } from "~/celeris/client.server";
import { planGateTask } from "~/celeris/decisions-admin.server";
import { promoteArtifact, readArtifactPromoteBody } from "~/celeris/docs-admin.server";
import { toActionError } from "~/celeris/errors";
import { runRetryAction, runTaskAction } from "~/celeris/route-actions.server";
import { loadTaskChanges, readTaskChangesQuery, type TaskChangesData } from "~/celeris/task-changes";
import { loadTaskFiles, readTaskFilesQuery, type TaskFilesData } from "~/celeris/task-files";
import {
  buildTaskEdit,
  commentOnTask,
  decomposeTask,
  editTask,
  phaseGateTask,
  reopenTask,
  rereviewTask,
} from "~/celeris/tasks-admin.server";
import type {
  ApprovalItem,
  ArtifactList,
  BrowserRun,
  BrowserWait,
  CommentList,
  ConfigView,
  Event,
  EventsPage,
  Inbox,
  MilestoneView,
  OrgList,
  OrgNode,
  ProjectDetail,
  TaskDetail,
  TaskRoutingView,
  TaskTreeView,
  Timeline,
} from "~/celeris/types";
import type { LiveViewState } from "~/components/BrowserRunsPanel";
import {
  activeBrowserRunIds,
  type BrowserOwnerView,
  liveViewLinkFor,
  liveViewPath,
  NO_BROWSER_OWNER,
  redactLiveViewUrls,
} from "~/lib/browser";
import { parseTaskTab } from "~/lib/labels";
import { milestoneTitle } from "~/lib/project-index";

export interface TaskDetailData {
  detail: TaskDetail;
  /** `live_view_url` は消してある（ADR-0080 D6）。 */
  browserRuns: BrowserRun[];
  /** ADR-0080 D5: browser の人待ち（非秘密）。この API を持たない celeris では空。 */
  browserWaits: BrowserWait[];
  /** ADR-0080 D6: この session が本人か（CSRF token は本人のときだけ）。 */
  browserOwner: BrowserOwnerView;
  /** run ごとの Live View の導線（URL は同一 origin の `/browser/live/...` だけ）。 */
  liveViews: Record<string, LiveViewState>;
  events: EventsPage;
  artifacts: ArtifactList;
  /** ADR-0044 D5: `GET /tasks/{id}/timeline`（時刻の昇順で 1 本）。 */
  timeline: Timeline;
  /** ADR-0044 D2: `GET /tasks/{id}/comments`（古い順）。タブの件数と、コメント欄の見出しに使う。 */
  comments: CommentList;
  /**
   * ADR-0055 D2 ラウンド 6（フェーズ 74）: タイムラインの相対時刻表示（`relativeTimeLabel`。
   * `~/routes/approvals.tsx` の `fetchedAt` と同じ作り）の基準時刻。loader が読み込んだ時刻。
   */
  fetchedAt: string;
  /** ADR-0044 D1: 編集フォームの「担当」プルダウンの選択肢（`GET /org`。落ちても画面は出す）。 */
  org: OrgNode[];
  /** ADR-0044 D1: 編集フォームの「途中目標」プルダウン（そのタスクの案件のものだけ）。 */
  milestones: MilestoneView[];
  /**
   * ADR-0046 D3（Phase 59）: 編集フォームの「ハーネス」プルダウンの選択肢（`GET /config` の
   * `genres[].id` = ハーネスのレジストリの射影）。空なら自由記述の欄にする（celeris 側で検証しない
   * 最小構成。`~/routes/org.tsx` の「分野」欄と同じ考え方）。落ちても画面は出す。
   */
  genres: string[];
  /**
   * ADR-0046 D5（Phase 59 / G21）: matching が担当を決めた理由（「なぜこの担当か」）。
   * `GET /tasks/{id}/events` に載っている最新の `assigned` イベント。無ければ null
   * （明示の `assignee` で作られた・組織を使っていない構成 等）。
   */
  assignedEvent: { node: string; score: number; reason: string } | null;
  /**
   * celeris ADR-0069 D5: なぜその担当・harness・lane・model か（`GET /tasks/{id}/routing`）。
   * 落ちたら（この API を持たない古い celeris 等）null でパネルを出さない。画面は落とさない。
   */
  routing: TaskRoutingView | null;
  /**
   * ADR-0043 D6 + ADR-0044 D5（Phase 52 + 53 のマージ）: 「ファイル」タブの中身。
   * **`?tab=files` のときだけ**引く（他のタブで毎回 `GET /tasks/{id}/tree` を叩かないため）。
   * 作業ツリーが無いタスク（404 `file_not_found`）やリポジトリを使わないタスクでは `error` に
   * celeris の文言が入り、タブはその文言だけを出す（ページ全体は落とさない）。
   */
  files: { data: TaskFilesData; error: null } | { data: null; error: ActionError } | null;
  /**
   * ADR-0043 D5 + ADR-0044 D5（Phase 53 + 54 のマージ）: 「変更」タブの中身。
   * **`?tab=changes` のときだけ**引く（`GET /tasks/{id}/changes` はリポジトリごとに git を数回起こすので、
   * 他のタブを見ているあいだは走らせない。celeris 側の U54-1）。ブランチも作業ツリーも無いタスクや
   * リポジトリを使わないタスクでは `error` に celeris の文言が入り、タブはその文言だけを出す。
   */
  changes: { data: TaskChangesData; error: null } | { data: null; error: ActionError } | null;
  /**
   * celeris ADR-0079 D14（Phase R4b）: 「木」タブの中身（`GET /tasks/{id}/task-tree?root=true`）。
   * **`?tab=tree` のときだけ**引く（節点ごとに events を読むので、他のタブでは叩かない）。失敗はタブの中に出す。
   */
  taskTree: { data: TaskTreeView; error: null } | { data: null; error: ActionError } | null;
  /**
   * 人のレビュー待ち（`reviewing` のタスク、または `kind = approval` のタスク）の判断材料。
   * `GET /inbox` の未決の承認のうち、このタスクが承認タスク自身か、その親のもの。落ちても画面は出す（空）。
   */
  humanReview: ApprovalItem[];
  /** どの案件・どの途中目標・誰の仕事か（監査 M2「裏方から戻れる」）。分からなければ null。 */
  place: {
    projectId: string | null;
    projectTitle: string | null;
    milestoneTitle: string | null;
    assigneeId: string | null;
    assigneeName: string | null;
  };
}

/**
 * `/tasks/:id`（タスク詳細、docs/DESIGN.md §4.3。ADR-0044 D5 でタブになった）の loader 本体。
 * `GET /tasks/{id}`・`GET /tasks/{id}/events`・`GET /tasks/{id}/artifacts`・`GET /tasks/{id}/timeline`・
 * `GET /tasks/{id}/comments` を並列に呼び、応答をそのまま返す（派生値は celeris 側で計算済み。GUI は再計算しない）。
 * celeris 停止中・タスクが無い（404 `task_not_found`）等は呼び出し側（`loader`）が `Response` に変換して投げる
 * （docs/adr/0004-g1-decisions.md D6。本番ビルドは素の Error を ErrorBoundary に渡す前に汎用 500 へ
 * サニタイズするため、`Response` として投げないと celeris 停止中でもバナーではなく 500 になってしまう）。
 * 生ログ本体は `/tasks/:id/runs/:runId`（別ルート）、DAG は `/graph`（docs/adr/0006-g3-decisions.md D4）。
 *
 * `GET /org` と案件の詳細は**編集フォームの選択肢**（ADR-0044 D1: 担当・途中目標のプルダウン）にも使うので、
 * 担当や案件が未設定でも常に引く（落ちたら選択肢が空になるだけ。画面は出す）。
 */
export async function loadTaskDetail(
  client: CelerisClient,
  taskId: string,
  request: Request,
  /** ADR-0080 D6: 本人状態は loader が `browserOwnerView(request)` で求めて渡す（既定は「本人を識別できない」）。 */
  browserOwner: BrowserOwnerView = NO_BROWSER_OWNER,
  /** Live View の relay が設定されているか（loader が `liveViewRelayAvailable()` で求めて渡す）。 */
  liveViewRelay = false,
): Promise<TaskDetailData> {
  const url = new URL(request.url);
  // フォームは `types` チェックボックスごとに 1 つずつ付ける（`?types=a&types=b`）。
  // celeris 側はカンマ区切りの単一パラメータを期待する（docs/celeris-api-v1.md §3.6）ので、ここで結合する。
  const types = url.searchParams.getAll("types");
  const [browserRuns, browserWaits, detail, events, artifacts, timeline, comments] = await Promise.all([
    loadBrowserRuns(client, taskId, request.signal).catch(() => []),
    loadTaskBrowserWaits(client, taskId, request.signal).catch(() => []),
    client.get<TaskDetail>(`/tasks/${taskId}`, { signal: request.signal }),
    client.get<EventsPage>(`/tasks/${taskId}/events`, {
      query: { types: types.length > 0 ? types.join(",") : undefined },
      signal: request.signal,
    }),
    client.get<ArtifactList>(`/tasks/${taskId}/artifacts`, { signal: request.signal }),
    client.get<Timeline>(`/tasks/${taskId}/timeline`, { signal: request.signal }),
    client.get<CommentList>(`/tasks/${taskId}/comments`, { signal: request.signal }),
  ]);
  // 案件・途中目標・担当の名前（監査 M2）と、編集フォームの選択肢（ADR-0044 D1、ADR-0046 D3 の「ハーネス」）。
  const assigneeId = detail.task.assignee ?? null;
  const projectId = detail.task.project_id ?? null;
  const [project, org, config, routing] = await Promise.all([
    projectId
      ? client
          .get<ProjectDetail>(`/projects/${encodeURIComponent(projectId)}`, { signal: request.signal })
          .catch(() => null)
      : Promise.resolve(null),
    client.get<OrgList>("/org", { signal: request.signal }).catch(() => null),
    client.get<ConfigView>("/config", { signal: request.signal }).catch(() => null),
    client.get<TaskRoutingView>(`/tasks/${taskId}/routing`, { signal: request.signal }).catch(() => null),
  ]);
  // ADR-0046 D5（Phase 59 / G21）: 「なぜこの担当か」。`assigned` は matching が決めたときに 1 件だけ
  // 付く（明示の assignee で作られたタスクには無い）。複数走っていれば直近を出す。
  const assignedEvent = events.items
    .map((row) => row.event)
    .filter((e): e is Extract<Event, { type: "assigned" }> => e.type === "assigned")
    .at(-1);
  // ADR-0043 D6: 「ファイル」タブを見ているときだけ作業ツリーを引く。403 / 404 は画面に出す
  // （兄弟のルート `/tasks/:id/files` はページ自体を落とすが、タブでは他のタブが見えていてほしい）。
  let files: TaskDetailData["files"] = null;
  if (parseTaskTab(url.searchParams.get("tab")) === "files") {
    try {
      files = { data: await loadTaskFiles(client, taskId, readTaskFilesQuery(request), request.signal), error: null };
    } catch (e) {
      files = { data: null, error: toActionError(e) };
    }
  }
  // ADR-0043 D5:「変更」タブを見ているときだけ差分を引く（`GET /tasks/{id}/changes` は git を起こす）。
  // 404 / 403 はタブの中に出す（兄弟のルート `/tasks/:id/changes` はページ自体を落とす）。
  let changes: TaskDetailData["changes"] = null;
  if (parseTaskTab(url.searchParams.get("tab")) === "changes") {
    try {
      changes = {
        data: await loadTaskChanges(client, taskId, readTaskChangesQuery(request), request.signal),
        error: null,
      };
    } catch (e) {
      changes = { data: null, error: toActionError(e) };
    }
  }
  // celeris ADR-0079 D14（Phase R4b）:「木」タブを見ているときだけ木を引く（root から。この task は印を付ける）。
  let taskTree: TaskDetailData["taskTree"] = null;
  if (parseTaskTab(url.searchParams.get("tab")) === "tree") {
    try {
      taskTree = {
        data: await client.get<TaskTreeView>(`/tasks/${taskId}/task-tree`, {
          query: { root: "true" },
          signal: request.signal,
        }),
        error: null,
      };
    } catch (e) {
      taskTree = { data: null, error: toActionError(e) };
    }
  }
  // 人のレビュー待ちの判断材料（`GET /inbox` の承認）。reviewing の親か承認タスク自身のときだけ引く。
  let humanReview: ApprovalItem[] = [];
  if (detail.task.status === "reviewing" || detail.task.kind === "approval") {
    const inbox = await client.get<Inbox>("/inbox", { signal: request.signal }).catch(() => null);
    humanReview = (inbox?.approvals ?? []).filter((a) => a.approval.id === taskId || a.parent?.id === taskId);
  }
  // ADR-0080 D6: 導線は本人・実行中・認証区間外のときだけ。dashboard URL は loader data に載せない。
  const activeRuns = activeBrowserRunIds(detail.runs, detail.task.status);
  const liveViews: Record<string, LiveViewState> = {};
  for (const run of browserRuns) {
    liveViews[run.run_id] = liveViewLinkFor(run, {
      isOwner: browserOwner.isOwner,
      ownerAvailable: browserOwner.available,
      active: activeRuns.includes(run.run_id),
      waits: browserWaits,
      relayAvailable: liveViewRelay,
      href: liveViewPath(taskId, run.run_id),
    });
  }
  return {
    detail,
    browserRuns: redactLiveViewUrls(browserRuns),
    browserWaits,
    browserOwner,
    liveViews,
    events: redactLiveViewUrls(events),
    artifacts,
    timeline: redactLiveViewUrls(timeline),
    comments,
    fetchedAt: new Date().toISOString(),
    org: org?.items ?? [],
    milestones: project?.milestones ?? [],
    genres: (config?.genres ?? []).map((g) => g.id),
    assignedEvent: assignedEvent
      ? { node: assignedEvent.node, score: assignedEvent.score, reason: assignedEvent.reason }
      : null,
    routing,
    files,
    changes,
    taskTree,
    humanReview,
    place: {
      projectId,
      projectTitle: project?.project.title ?? null,
      milestoneTitle: milestoneTitle(project?.milestones ?? [], detail.task.milestone_id),
      assigneeId,
      assigneeName: assigneeId ? (org?.items.find((n) => n.id === assigneeId)?.name ?? assigneeId) : null,
    },
  };
}

/**
 * `/tasks/:id` の action 本体（intent ごとの振り分け）。ルートの `action` は `formData()` を読んで渡すだけ。
 */
export async function runTaskDetailAction(client: CelerisClient, taskId: string, form: FormData, signal: AbortSignal) {
  const intent = form.get("intent");
  // Phase 31: `retry` は `approve`/`reject`/`answer`/`cancel`（`TransitionInput`）とは語彙も応答の形も別
  // （新しいタスクを作る。`RetryOutcome`）なので、共通の `readTransitionForm` に渡す前に分岐する。
  // Phase 53（ADR-0044 D1/D2）: `edit` / `comment` / `reopen` も同じ理由でここで分ける。
  if (intent === "retry") {
    const outcome = await runRetryAction(client, taskId, form, signal);
    return retryData(outcome);
  }
  if (intent === "edit") {
    const outcome = await editTask(client, taskId, buildTaskEdit(form), signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  if (intent === "comment") {
    const outcome = await commentOnTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 201 : outcome.error.status });
  }
  if (intent === "reopen") {
    const outcome = await reopenTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  // ADR-0070 D2（Phase 116）: 再レビューは `retry`/`reopen` と同じ理由でここで分ける
  // （`TransitionInput` とは語彙が違う。`task_ops::comment::rereview` を呼ぶ）。
  if (intent === "rereview") {
    const outcome = await rereviewTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  // ADR-0044 D7（Phase 57 / G20）: 成果物を案件の文書に昇格する（**管理系**。宛先は人が決める）。
  if (intent === "promote") {
    const outcome = await promoteArtifact(client, taskId, readArtifactPromoteBody(form), signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  // celeris ADR-0074 D2.4（Phase F3 途中確認）: 途中確認への応答（`POST /tasks/{id}/execution/phase-gate`）。
  if (intent === "phase_gate") {
    const outcome = await phaseGateTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  // celeris ADR-0079 D8（Phase R4b）: root の計画の承認（`POST /tasks/{id}/execution/plan-gate`）。
  // 受信箱の「計画の承認」の 3 つのボタンもここへ送る。
  if (intent === "plan_gate") {
    const outcome = await planGateTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  // celeris ADR-0072「Phase F6 実装時の決定」: 起票済みのタスクの実行の形を決め直す
  // （`POST /tasks/{id}/execution/decompose`）。終端のタスクの「計画を作らせてやり直す」は `retry` の `execution`。
  if (intent === "execution_decompose") {
    const outcome = await decomposeTask(client, taskId, form, signal);
    return data(outcome, { status: outcome.ok ? 200 : outcome.error.status });
  }
  const outcome = await runTaskAction(client, taskId, form, signal);
  return transitionData(outcome);
}
