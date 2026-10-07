// SSE フレーム → invalidate する key の集合（ADR-0081 D6 の表）。
// task.event の種類ごとの対応は `Record<EventKind, KindSpec>` で、生成型に種類が増えると型検査で落ちる。
// 実行時には schema.json と突き合わせるテストも落とす。
//
// D6 の記号: T=task detail+timeline、L=tasks/list・inbox・board、P=project 集計、N=reports・approvals・daemon/rest、
// R=task の runs と run 群、E=task の execution。timeline は全 task.event で対象 task だけ stale にする。
// 表に無い 14 種（browser_*, decision_*, child_*, cos_operation, work_unit_spec_overridden, unit_gate_overridden,
// plan_approval_requested, stall_detected, delivery_skipped）の範囲は web ADR-W1 に記録。

import type { QueryClient, QueryKey } from "@tanstack/react-query";
import type { EventRow } from "../generated/types";
import {
  accountKeys,
  approvalKeys,
  boardKeys,
  clusterKeys,
  daemonKeys,
  inboxKeys,
  metricKeys,
  modelKeys,
  notificationKeys,
  projectKeys,
  providerKeys,
  reportKeys,
  taskKeys,
} from "../queries/keys";
import type { EventKind } from "./event-kinds";
import type { SignalFrame } from "./frames";

export type { EventKind } from "./event-kinds";

/** キーの無い成果物一覧（D6: `['artifacts',filters]`）と run のログ（`['tasks','log',taskId,runId,name,offset]`）。 */
export const artifactListKey = ["artifacts"] as const;
export const taskLogKey = (taskId: string, runId?: string): QueryKey =>
  runId === undefined ? ["tasks", "log", taskId] : ["tasks", "log", taskId, runId];

type Token =
  | "T"
  | "L"
  | "P"
  | "N"
  | "R"
  | "E"
  | "files"
  | "changes"
  | "artifacts"
  | "metrics"
  | "clusters"
  | "providers"
  | "accounts"
  | "llmModels"
  | "daemonRest";

export type EventContext = {
  taskId: string;
  event: EventRow["event"];
  /** event の明示値または Query cache から解決した所属。null は不明、false は案件に属さない。 */
  projectId: string | false | null;
};

type KindSpec = {
  sets: readonly Token[];
  /** 所属が変わり得る編集。旧・新が分からないので project 集計の fallback を使う。 */
  projectMayChange?: true;
  extra?: (ctx: EventContext) => QueryKey[];
};

const TLE = ["T", "E", "R", "L", "P"] as const;

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}

function detailAndTimeline(taskId: string): QueryKey[] {
  return [taskKeys.detail(taskId), taskKeys.timelines(taskId)];
}

export const EVENT_INVALIDATION: Record<EventKind, KindSpec> = {
  created: { sets: ["T", "L", "P"] },
  transitioned: { sets: ["T", "L", "P", "N", "E"] },
  worker_started: { sets: ["T", "R", "E", "L", "P"] },
  // 対象 run の progress/log と対象 task の timeline だけ。一覧・project・設定は取り直さない。
  worker_progress: { sets: [], extra: ({ taskId, event }) => runScoped(taskId, event) },
  // ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: 対象 run の progress/log と task の timeline だけ（worker_progress と同じ範囲）。
  worker_policy_violation: { sets: [], extra: ({ taskId, event }) => runScoped(taskId, event) },
  artifact_produced: { sets: ["T", "R", "artifacts", "files", "P"], extra: () => [artifactListKey] },
  worker_finished: { sets: ["T", "R", "E", "L", "P", "N", "changes", "files", "artifacts", "metrics"] },
  review_verdict: { sets: ["T", "R", "E", "L"] },
  approval_requested: { sets: ["T", "L", "N", "E", "P"] },
  approval_decided: { sets: ["T", "L", "N", "E", "P"] },
  approvals_withdrawn: { sets: ["T", "L", "N", "E", "P"] },
  answered: { sets: ["T", "L", "N", "R", "E"] },
  question_raised: { sets: ["T", "L", "N", "R", "E"] },
  delegated: {
    sets: TLE,
    extra: ({ event }) =>
      event.type === "delegated" ? stringArray(event.task_ids).map((id) => taskKeys.detail(id)) : [],
  },
  cluster_unavailable: { sets: ["T", "L", "clusters", "daemonRest"] },
  cluster_master_exited: { sets: ["T", "L", "clusters", "daemonRest"] },
  provider_throttled: { sets: ["T", "providers", "accounts", "daemonRest"] },
  retried: {
    sets: ["T", "L", "P"],
    extra: ({ event }) => (event.type === "retried" ? detailAndTimeline(String(event.from)) : []),
  },
  edited: { sets: ["T", "L", "P", "E"], projectMayChange: true },
  assigned: { sets: ["T", "L", "P", "E"], projectMayChange: true },
  workspace_mode_downgraded: { sets: ["T", "files", "changes", "artifacts", "metrics"] },
  workspace_pruned: { sets: ["T", "files", "changes", "artifacts", "metrics"] },
  routing_decided: { sets: ["T", "R"] },
  // ADR 2026-10-04-multi-objective-model-routing Phase 3: dispatch の特徴 snapshot・proxy の要求単位
  // trace・run 単位の outcome。いずれも対象 task の routing 監査（R）だけを古くする。
  routing_features_recorded: { sets: ["T", "R"] },
  routing_request_decided: { sets: ["T", "R"] },
  routing_outcome_recorded: { sets: ["T", "R"] },
  // Phase 4: shadow 監査（primary とは別欄）。対象 task の routing 監査（R）だけを古くする。
  routing_shadow_recorded: { sets: ["T", "R"] },
  checkpoint_saved: { sets: ["T", "R", "E"] },
  execution_planned: { sets: TLE },
  work_unit_transitioned: { sets: TLE },
  execution_gated: { sets: TLE },
  execution_routed: { sets: ["T", "E"] },
  execution_hint_set: { sets: TLE },
  repair_scheduled: { sets: TLE },
  quota_estimated: { sets: ["T", "R", "E", "accounts", "providers", "metrics", "P"] },
  work_unit_committed: { sets: ["T", "E", "changes", "files", "artifacts", "P"] },
  phase_integrated: { sets: ["T", "E", "changes", "files", "artifacts", "P"] },
  work_units_serialized: { sets: ["T", "E"] },
  pause_points_resolved: { sets: ["T", "E"] },
  phase_reported: { sets: ["T", "E", "artifacts", "L", "N", "P"] },
  // 明示 project_id の detail/plan/tasks/list 集計は projectKeysFor が引く。
  project_plan_proposed: { sets: ["T", "L", "N", "P"] },
  project_plan_decided: { sets: ["T", "L", "N", "P"] },
  // --- schema にあり D6 の表に無い種類（隣接する種類にならう） ---
  browser_updated: { sets: ["T", "R", "N"], extra: () => [["browser"]] },
  browser_wait_opened: { sets: ["T", "R", "N"], extra: () => [["browser"]] },
  browser_wait_resolved: { sets: ["T", "R", "N"], extra: () => [["browser"]] },
  cluster_job_wait_started: { sets: ["T", "R", "E", "L"] },
  cluster_job_wait_polled: { sets: ["T", "R", "E"] },
  cluster_job_wait_finished: { sets: ["T", "R", "E", "L"] },
  work_unit_spec_overridden: { sets: ["T", "E", "R", "L", "P"] },
  work_unit_checks_failed: { sets: ["T", "E", "R", "L", "P"] },
  unit_gate_overridden: { sets: ["T", "E", "R", "L", "P"] },
  child_task_created: {
    sets: ["T", "E", "R", "L", "P"],
    extra: ({ event }) => (event.type === "child_task_created" ? [taskKeys.detail(String(event.child_task_id))] : []),
  },
  cos_operation: { sets: ["T"] },
  child_adopted: {
    sets: ["T", "E", "R", "L", "P"],
    extra: ({ event }) => (event.type === "child_adopted" ? [taskKeys.detail(String(event.child_task_id))] : []),
  },
  decision_requested: { sets: ["T", "L", "N"] },
  decision_answered: { sets: ["T", "L", "N"] },
  decision_withdrawn: { sets: ["T", "L", "N"] },
  plan_approval_requested: { sets: ["T", "L", "N", "E"] },
  stall_detected: { sets: ["T", "R", "L"] },
  // ADR-0118 / ADR-0120: review 前同期と integration repair（TaskDetail・受信箱の Failed 項目に出る）。
  review_target_synced: { sets: ["T", "E"] },
  review_target_advanced: { sets: ["T", "E"] },
  merge_candidate_stale: { sets: ["T", "E", "L"] },
  integration_repair_scheduled: { sets: TLE },
  integration_repair_resolved: { sets: TLE },
  integration_repair_exhausted: { sets: TLE },
  delivery_skipped: { sets: ["T", "L", "N"] },
  // ADR-0131 付記（2026-10-04）: 日次整理の適用・commit・push の記録。報告は別に 1 件ある。
  knowledge_curation_applied: { sets: ["T", "N"] },
  // ADR 2026-10-06 opencode-go-and-model-catalog D4: catalog の入れ替わり。モデル一覧と providers（routing catalog）を古くする。
  model_catalog_changed: { sets: ["llmModels", "providers"] },
  // ADR 2026-10-06 model-role-assignments D2: 役割の割り当ての変更。assignments と models は同じ key 配下（llm-models）。
  model_role_assignment_changed: { sets: ["llmModels", "providers"] },
  integration_requested: { sets: ["L"] },
  integration_answered: { sets: ["L"] },
  // 統合 WU の検査の開始・終了（TaskDetail の WU 行の check_progress）。
  integration_check_started: { sets: ["T", "E"] },
  integration_check_finished: { sets: ["T", "E"] },
  // 葉の WU の受け入れ検査の開始・終了と、draining の旧 instance の手放し（同じ WU 行の check_progress）。
  work_unit_check_started: { sets: ["T", "E"] },
  work_unit_check_finished: { sets: ["T", "E"] },
  work_unit_checks_handed_off: { sets: ["T", "E"] },
};

/**
 * SSE の軽い合図（ADR-0133 D5）→ invalidate する key。受信箱は旧 `GET /inbox` と `/inbox/items` を含む
 * `['inbox']` 全体、通知は一覧と未読数を含む `['notifications']` 全体。互いの key には触れない。
 */
export const SIGNAL_INVALIDATION: Readonly<Record<SignalFrame["type"], readonly QueryKey[]>> = {
  inbox_changed: [inboxKeys.all],
  notifications_changed: [notificationKeys.all],
};

function runScoped(taskId: string, event: EventRow["event"]): QueryKey[] {
  const runId = "run_id" in event && typeof event.run_id === "string" ? event.run_id : undefined;
  return [
    taskKeys.timelines(taskId),
    runId === undefined ? taskKeys.runsOf(taskId) : taskKeys.run(taskId, runId),
    taskLogKey(taskId, runId),
  ];
}

/** 所属不明のときの project 集計（docs は除く）。 */
export function projectFallbackKeys(): QueryKey[] {
  return [projectKeys.lists(), ["projects", "detail"], ["projects", "tasks"], ["projects", "plan"]];
}

function projectKeysFor(projectId: string | null | false): QueryKey[] {
  if (projectId === false) return [];
  if (projectId === null) return projectFallbackKeys();
  return [
    projectKeys.lists(),
    projectKeys.detail(projectId),
    projectKeys.tasksOf(projectId),
    projectKeys.plan(projectId),
  ];
}

const TOKEN_KEYS: Record<Exclude<Token, "P">, (taskId: string) => QueryKey[]> = {
  T: (id) => detailAndTimeline(id),
  L: () => [taskKeys.lists(), inboxKeys.all, boardKeys.all],
  N: () => [reportKeys.all, approvalKeys.all, daemonKeys.rest()],
  R: (id) => [taskKeys.runs(id), taskKeys.runsOf(id)],
  E: (id) => [taskKeys.execution(id)],
  files: (id) => [taskKeys.files(id)],
  changes: (id) => [taskKeys.changes(id)],
  artifacts: (id) => [taskKeys.artifacts(id)],
  metrics: () => [metricKeys.all],
  clusters: () => [clusterKeys.all],
  providers: () => [providerKeys.all],
  accounts: () => [accountKeys.all],
  llmModels: () => [modelKeys.all],
  daemonRest: () => [daemonKeys.rest()],
};

/** task.event 1 件で stale にする key の集合（重複は除く）。timeline は全種類で対象 task だけ。 */
export function keysForTaskEvent(ctx: EventContext): QueryKey[] {
  const spec = EVENT_INVALIDATION[ctx.event.type];
  const out: QueryKey[] = [taskKeys.timelines(ctx.taskId)];
  for (const token of spec.sets) {
    if (token === "P") {
      out.push(...projectKeysFor(spec.projectMayChange ? null : ctx.projectId));
    } else {
      out.push(...TOKEN_KEYS[token](ctx.taskId));
    }
  }
  const explicit = explicitProjectId(ctx.event);
  if (explicit !== null && (ctx.event.type === "project_plan_proposed" || ctx.event.type === "project_plan_decided")) {
    out.push(...projectKeysFor(explicit));
  }
  if (spec.extra) out.push(...spec.extra(ctx));
  return dedupeKeys(out);
}

export function dedupeKeys(keys: readonly QueryKey[]): QueryKey[] {
  const seen = new Set<string>();
  const out: QueryKey[] = [];
  for (const key of keys) {
    const id = JSON.stringify(key);
    if (seen.has(id)) continue;
    seen.add(id);
    out.push(key);
  }
  return out;
}

/** event 自身が持つ project_id（H1: created.task.project_id と project_plan_*）。 */
export function explicitProjectId(event: EventRow["event"]): string | null {
  if (event.type === "created") {
    const projectId = (event.task as { project_id?: unknown } | undefined)?.project_id;
    return typeof projectId === "string" ? projectId : null;
  }
  if (event.type === "project_plan_proposed" || event.type === "project_plan_decided") return event.project_id;
  return null;
}

function rowsOf(data: unknown): unknown[] {
  if (Array.isArray(data)) return data;
  if (typeof data === "object" && data !== null) {
    const items = (data as { items?: unknown }).items;
    if (Array.isArray(items)) return items;
  }
  return [];
}

/** task の所属。project id、属さないと分かれば false、cache に無ければ null。 */
function projectOfTask(data: unknown, taskId: string): string | false | null {
  if (typeof data === "object" && data !== null) {
    // ProjectDetail（P4-02）: 案件の task 一覧に含まれていればその案件。
    const detail = data as { project?: { id?: unknown }; tasks?: unknown };
    if (typeof detail.project?.id === "string" && Array.isArray(detail.tasks)) {
      const owner = detail.project.id;
      if (detail.tasks.some((row) => (row as { id?: unknown } | null)?.id === taskId)) return owner;
    }
  }
  const candidates = [data, ...rowsOf(data)];
  for (const row of candidates) {
    if (typeof row !== "object" || row === null) continue;
    const r = row as { id?: unknown; project_id?: unknown; title?: unknown };
    if (r.id !== taskId) continue;
    if (typeof r.project_id === "string") return r.project_id;
    // task の行（title を持つ）で project_id が無い → どの案件にも属さない。
    if (typeof r.title === "string") return false;
  }
  return null;
}

/**
 * H1: project の解決。event の明示値 → Query cache の task detail / 一覧 / project の task 一覧。
 * 別の永続対応表は作らない。分からなければ null（呼び出し側が集計 key の fallback を使う）。
 */
export function resolveProjectId(
  queryClient: QueryClient,
  taskId: string,
  event: EventRow["event"],
): string | false | null {
  const explicit = explicitProjectId(event);
  if (explicit !== null) return explicit;
  const fromDetail = projectOfTask(queryClient.getQueryData(taskKeys.detail(taskId)), taskId);
  if (fromDetail !== null) return fromDetail;
  let none = false;
  for (const root of [projectKeys.all, taskKeys.lists()]) {
    for (const [, data] of queryClient.getQueriesData({ queryKey: root })) {
      const found = projectOfTask(data, taskId);
      if (typeof found === "string") return found;
      if (found === false) none = true;
    }
  }
  return none ? false : null;
}
