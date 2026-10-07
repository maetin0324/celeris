// ADR 2026-10-06 model-role-assignments D4: source × 役割（frontier / standard / cheap）→ model の割り当て API の client。
// 生成型（api/generated/types.ts）にはまだ無いので型はここに持つ。生成型が入ったら差し替える。
// path は既存の models client と同じ `/api/llm/models/...`（gateway が `/api/v1` へ写す）。

import { apiGet, apiMutate } from "./client";

export type AssignmentTier = "frontier" | "standard" | "cheap";
export const ASSIGNMENT_TIERS: readonly AssignmentTier[] = ["frontier", "standard", "cheap"];

export type EffectiveAssignmentView = {
  source: string;
  tier: AssignmentTier;
  model_id: string;
  priority?: number;
  state: "assigned" | "excluded";
  excluded_reason: string | null;
  note: string | null;
  updated_at: string;
  updated_by: string;
};

export type RoleSlotView = {
  source: string;
  tier: AssignmentTier;
  model_id: string | null;
  priority?: number;
  origin: "assignment" | "config" | null;
  excluded_reason: string | null;
  providers: string[];
  proxy: boolean;
  available: boolean | null;
  last_seen: string | null;
};

export type ImpactChange = {
  kind: "provider" | "proxy";
  id: string;
  tier: string;
  before: string | null;
  after: string | null;
  excluded_reason: string | null;
};
export type ImpactView = { changes: ImpactChange[] };

export type AssignmentList = { items: EffectiveAssignmentView[]; effective: RoleSlotView[] };
export type AssignmentPut = { item: EffectiveAssignmentView; impact: ImpactView };

const BASE = "/api/llm/models/assignments";

export function assignmentPath(source: string, tier: string): string {
  return `${BASE}/${encodeURIComponent(source)}/${encodeURIComponent(tier)}`;
}
export const assignmentsPreviewPath = `${BASE}/preview`;
export const ASSIGNMENTS_LIST_PATH = BASE;

export function getAssignments(signal?: AbortSignal): Promise<AssignmentList> {
  return apiGet<AssignmentList>(BASE, signal);
}

export function putAssignment(
  source: string,
  tier: AssignmentTier,
  body: { model_id: string; note?: string },
): Promise<AssignmentPut> {
  return apiMutate<AssignmentPut>("PUT", assignmentPath(source, tier), body);
}

export function deleteAssignment(source: string, tier: AssignmentTier): Promise<void> {
  return apiMutate<void>("DELETE", assignmentPath(source, tier));
}

export function previewAssignment(body: {
  source: string;
  tier: AssignmentTier;
  model_id: string | null;
}): Promise<{ impact: ImpactView }> {
  return apiMutate<{ impact: ImpactView }>("POST", assignmentsPreviewPath, body);
}
