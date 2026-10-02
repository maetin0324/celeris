// task.event の event.type の一覧（生成型 Event["type"] から導く）。
// schema に種類が増えると `satisfies` と網羅検査（下の型）がコンパイル時に落ちる。
// 実行時には schema.json と突き合わせるテストがある（invalidation-map.test.ts）。

import type { Event } from "../generated/types";

export type EventKind = Event["type"];

export const EVENT_KINDS = [
  "browser_updated",
  "browser_wait_opened",
  "browser_wait_resolved",
  "cluster_job_wait_started",
  "cluster_job_wait_polled",
  "cluster_job_wait_finished",
  "created",
  "transitioned",
  "worker_started",
  "worker_progress",
  "artifact_produced",
  "worker_finished",
  "review_verdict",
  "approval_requested",
  "approval_decided",
  "approvals_withdrawn",
  "answered",
  "question_raised",
  "delegated",
  "cluster_unavailable",
  "cluster_master_exited",
  "provider_throttled",
  "retried",
  "edited",
  "assigned",
  "workspace_mode_downgraded",
  "workspace_pruned",
  "routing_decided",
  "checkpoint_saved",
  "execution_planned",
  "work_unit_transitioned",
  "work_unit_checks_failed",
  "work_unit_spec_overridden",
  "execution_gated",
  "execution_hint_set",
  "repair_scheduled",
  "quota_estimated",
  "work_unit_committed",
  "phase_integrated",
  "work_units_serialized",
  "pause_points_resolved",
  "phase_reported",
  "project_plan_proposed",
  "project_plan_decided",
  "child_task_created",
  "child_adopted",
  "unit_gate_overridden",
  "decision_requested",
  "decision_answered",
  "decision_withdrawn",
  "plan_approval_requested",
  "stall_detected",
  "review_target_synced",
  "review_target_advanced",
  "merge_candidate_stale",
  "integration_repair_scheduled",
  "integration_repair_resolved",
  "integration_repair_exhausted",
] as const satisfies readonly EventKind[];

/** EVENT_KINDS が EventKind を漏れなく含むことの型検査（漏れがあると never でなくなり代入できない）。 */
type Missing = Exclude<EventKind, (typeof EVENT_KINDS)[number]>;
const _exhaustive: [Missing] extends [never] ? true : never = true;
void _exhaustive;

const KIND_SET: ReadonlySet<string> = new Set(EVENT_KINDS);

export function isEventKind(value: unknown): value is EventKind {
  return typeof value === "string" && KIND_SET.has(value);
}
