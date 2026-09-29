import { readWorkspaceFromForm } from "~/lib/workspace-form";
import type { CreateFailure, ProjectOpOutcome } from "./action-types";
import { toActionError } from "./actions.server";
import type { CelerisClient } from "./client.server";
import { formString } from "./forms";
import type {
  Project,
  ProjectCreateBody,
  ProjectLifecycle,
  ProjectPatchBody,
  ProjectStatus,
  WorkspaceSpec,
} from "./types";

/**
 * 「案件」画面（`/projects`, `/projects/:id`）からの中継（ADR-0033 D2、docs/celeris-api-v1.md §3.46〜3.49）。
 * `POST /projects`（案件の作成）は Phase 27（M-4）で**管理系**になった（`token_file` 未設定でも 401。
 * v1 の破壊的変更、docs/gui/api.md 冒頭の変更点一覧）。Phase 55（ADR-0044 D6）で
 * `PATCH /projects/{id}` / `POST /projects/{id}/milestones` / `PATCH /milestones/{id}` も管理系になり、
 * 中止・一時停止・アーカイブ（§3.84〜3.91）も最初から管理系。`GET /projects` だけが通常の要求。
 * celeris ADR-0079 D13（Phase R5a）: 案件計画（`POST /projects/{id}/plan`・`…/project-plan/{version}/decide`）と
 * 途中目標の書き込み（作成・状態・判定・中止・一時停止・再開）は 410 になったので、その中継は外した。
 * 管理系かどうかで GUI 側の中継コードは変わらない（`CelerisClient` はどちらも同じ `Authorization` ヘッダを
 * 付けるだけ。401 の案内文も `Flash.tsx` の `error.code === "unauthorized"` の 1 か所に集約されている）。
 * GUI 側では検証しない: celeris が 401 / 404 / 409 / 422 / 400 を返したらその文言をそのまま画面に出す。
 */

/** `POST /projects`（管理系）。`title` / `request` は空でもそのまま送り、celeris の 422 文言を出す（ADR-0005 D5）。 */
export async function createProject(
  client: CelerisClient,
  input: ProjectCreateBody,
  signal?: AbortSignal,
): Promise<{ ok: true; project: Project } | CreateFailure> {
  try {
    const project = await client.post<Project>("/projects", input, { signal });
    return { ok: true, project };
  } catch (e) {
    return { ok: false, error: toActionError(e) };
  }
}

/**
 * フォーム（`title` / `request` / `workspace_kind` / `workspace_path` / `workspace_cluster`）から
 * `ProjectCreateBody` を読む。作業場所が「まだ決めない」（省略を含む）なら `workspace` キー自体を送らない
 * （docs/celeris-api-v1.md §3.46「省略すれば従来どおり作業場所なし」。ADR-0039 D1、Phase G13k）。
 */
export function readProjectCreateInput(form: FormData): ProjectCreateBody {
  const body: ProjectCreateBody = {
    title: formString(form, "title") ?? "",
    request: formString(form, "request") ?? "",
  };
  const workspace = readWorkspaceFromForm(form);
  if (workspace) body.workspace = workspace;
  return body;
}

/** `PATCH /projects/{id}`（案件の状態変更。ADR-0033 D2 の `proposed`/`active`/`paused`/`done`）。 */
export async function patchProjectStatus(
  client: CelerisClient,
  id: string,
  status: ProjectStatus,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  try {
    const body: ProjectPatchBody = { status };
    const project = await client.patch<Project>(`/projects/${encodeURIComponent(id)}`, body, { signal });
    return { ok: true, op: "project_status", project };
  } catch (e) {
    return { ok: false, op: "project_status", error: toActionError(e) };
  }
}

/**
 * `PATCH /projects/{id}`（案件の作業場所だけを変える。ADR-0039 D1、Phase G13k）。`workspace = null` を
 * 明示すると「作業場所なし」に戻す（消去。docs/celeris-api-v1.md §3.48）。`status` は送らない（変えない）。
 */
export async function patchProjectWorkspace(
  client: CelerisClient,
  id: string,
  workspace: WorkspaceSpec | null,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  try {
    const body: ProjectPatchBody = { workspace };
    const project = await client.patch<Project>(`/projects/${encodeURIComponent(id)}`, body, { signal });
    return { ok: true, op: "project_workspace", project };
  } catch (e) {
    return { ok: false, op: "project_workspace", error: toActionError(e) };
  }
}

/**
 * celeris ADR-0072「Phase F6 実装時の決定」: 案件の名前（`title`）・説明（依頼文 `request`）・slug を変える
 * （`PATCH /projects/{id}`、**管理系**）。フォームの値をそのまま写す（前後の空白・空・長さの検証は celeris。
 * 422 の文言をそのまま画面に出す）。slug は今の値（hidden `slug_current`）と違うときだけ送る（空なら送らない）。
 */
export async function patchProjectText(
  client: CelerisClient,
  id: string,
  form: FormData,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  const body: ProjectPatchBody = {};
  const title = form.get("title");
  if (typeof title === "string") body.title = title;
  const request = form.get("request");
  if (typeof request === "string") body.request = request;
  const slug = formString(form, "slug");
  if (slug !== null && slug.trim() !== "" && slug.trim() !== (formString(form, "slug_current") ?? "")) {
    body.slug = slug.trim();
  }
  try {
    const project = await client.patch<Project>(`/projects/${encodeURIComponent(id)}`, body, { signal });
    return { ok: true, op: "project_edit", project };
  } catch (e) {
    return { ok: false, op: "project_edit", error: toActionError(e) };
  }
}

/**
 * 中止・一時停止・アーカイブ（ADR-0044 D6、docs/celeris-api-v1.md §3.84〜3.91。Phase 55 / G19。
 * **管理系**: `token_file` 未設定でも 401）。要求本文は `{}`（空の本体）で、応答はどれも 200。
 *
 * GUI 側では判断しない: **いまの状態でその操作ができるか**は celeris が決める
 * （中止済みの `cancel` / 終端・一時停止中の `pause` / `paused` でないものの `resume` /
 * 非終端の案件の `archive` は 409 `invalid_transition`）。`~/lib/lifecycle.ts` は押せないボタンを
 * 最初から出さないための**表示の判定だけ**で、押されたら常に celeris に送り、409 の文言をそのまま出す。
 * `archive` / `unarchive` は冪等（既にその状態なら 200）なので、二度押しは 409 にならない。
 * 連鎖で止まったもの（`cancelled_tasks` / `cancelled_milestones`）は celeris が返した配列をそのまま画面に渡す。
 */

/** 案件の操作 1 つを中継する（`cancel` / `pause` / `resume` / `archive` / `unarchive` で形が同じ）。 */
async function projectLifecycle(
  client: CelerisClient,
  op: "project_cancel" | "project_pause" | "project_resume" | "project_archive" | "project_unarchive",
  path: string,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  try {
    const lifecycle = await client.post<ProjectLifecycle>(
      `/projects/${encodeURIComponent(projectId)}/${path}`,
      {},
      { signal },
    );
    return { ok: true, op, lifecycle };
  } catch (e) {
    return { ok: false, op, error: toActionError(e) };
  }
}

/** `POST /projects/{id}/cancel`（§3.84）。属する非終端タスクと途中目標が連鎖で `cancelled` になる。 */
export function cancelProject(
  client: CelerisClient,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  return projectLifecycle(client, "project_cancel", "cancel", projectId, signal);
}

/** `POST /projects/{id}/pause`（§3.85）。属するタスクは dispatch されない（走っている run は最後まで走る）。 */
export function pauseProject(
  client: CelerisClient,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  return projectLifecycle(client, "project_pause", "pause", projectId, signal);
}

/** `POST /projects/{id}/resume`（§3.86）。`paused_from` へ戻す（戻り先を決めるのは celeris）。 */
export function resumeProject(
  client: CelerisClient,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  return projectLifecycle(client, "project_resume", "resume", projectId, signal);
}

/** `POST /projects/{id}/archive`（§3.87）。終端（`done` / `cancelled`）の案件だけ。冪等。 */
export function archiveProject(
  client: CelerisClient,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  return projectLifecycle(client, "project_archive", "archive", projectId, signal);
}

/** `POST /projects/{id}/unarchive`（§3.88）。冪等。 */
export function unarchiveProject(
  client: CelerisClient,
  projectId: string,
  signal?: AbortSignal,
): Promise<ProjectOpOutcome> {
  return projectLifecycle(client, "project_unarchive", "unarchive", projectId, signal);
}
