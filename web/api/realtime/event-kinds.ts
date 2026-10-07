// task.event の event.type の一覧（生成型 Event["type"] から導く）。
// schema に種類が増えると `satisfies` と網羅検査（下の型）がコンパイル時に落ちる。
// 実行時には schema.json と突き合わせるテストがある（invalidation-map.test.ts）。

import type { Event } from "../generated/types";

/**
 * 生成型（schema.json）に未反映の種類。Rust 側が schema を再生成したら `Event["type"]` に含まれるので、この一覧から消す
 * （消すまでは重複しても union は変わらない）。ADR 2026-10-06 model-role-assignments D2。
 */
export const PENDING_GENERATED_KINDS = [] as const;

export type EventKind = Event["type"] | (typeof PENDING_GENERATED_KINDS)[number];

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
  "worker_policy_violation",
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
  "routing_features_recorded",
  "routing_request_decided",
  "routing_outcome_recorded",
  "routing_shadow_recorded",
  "checkpoint_saved",
  "execution_planned",
  "work_unit_transitioned",
  "work_unit_checks_failed",
  "work_unit_spec_overridden",
  "execution_gated",
  "execution_routed",
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
  "delivery_skipped",
  "knowledge_curation_applied",
  "integration_requested",
  "integration_answered",
  "integration_check_started",
  "integration_check_finished",
  "work_unit_check_started",
  "work_unit_check_finished",
  "work_unit_checks_handed_off",
  "model_catalog_changed",
  "model_role_assignment_changed",
] as const satisfies readonly EventKind[];

/** EVENT_KINDS が EventKind を漏れなく含むことの型検査（漏れがあると never でなくなり代入できない）。 */
type Missing = Exclude<EventKind, (typeof EVENT_KINDS)[number]>;
const _exhaustive: [Missing] extends [never] ? true : never = true;
void _exhaustive;

const KIND_SET: ReadonlySet<string> = new Set(EVENT_KINDS);

export function isEventKind(value: unknown): value is EventKind {
  return typeof value === "string" && KIND_SET.has(value);
}
