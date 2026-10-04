import type { AttentionItem, IntegrationRepairExhaustReason, IntegrationRepairView } from "../../api/generated/types";
import type { BadgeTone } from "../../components/ui/badge";

// ADR-0120 D5: review 前同期の衝突解消（IntegrationRepair）の表示文。実装失敗（failure・Failed の
// reason/class）とは別物なので、見出し・ラベル・色を分ける。null/欠落なら何も出さない。

export const INTEGRATION_REPAIR_HEADING = "target drift に伴う integration repair";
export const INTEGRATION_REPAIR_LABEL = "統合修復（実装失敗ではない）";

const stateLabels: Record<IntegrationRepairView["state"], string> = {
  scheduled: "修復中（scheduled）",
  resolved: "解消済み（resolved）",
  exhausted: "打ち切り（exhausted）",
};

const reasonLabels: Record<IntegrationRepairExhaustReason, string> = {
  limit_reached: "上限回数に達した",
  plan_issue: "計画の問題",
  work_unit_failed: "修復 WU が失敗",
  budget_exhausted: "予算切れ",
  result_untrusted: "成果を信頼できない",
  abort_failed: "中断に失敗",
  worktree_unavailable: "作業ツリーが使えない",
};

export type IntegrationRepairTone = "info" | "success" | "warning";

export type IntegrationRepairRow = { label: string; value: string };

export type IntegrationRepairDisplay = {
  heading: string;
  label: string;
  state: IntegrationRepairView["state"];
  stateLabel: string;
  tone: IntegrationRepairTone;
  rows: IntegrationRepairRow[];
};

const tones: Record<IntegrationRepairView["state"], IntegrationRepairTone> = {
  scheduled: "info",
  resolved: "success",
  exhausted: "warning",
};

// 木の行の Badge の色。修復中は running、解消は success、打ち切りは warning（実装失敗の danger は使わない）。
const badgeTones: Record<IntegrationRepairTone, BadgeTone> = {
  info: "running",
  success: "success",
  warning: "warning",
};

export function integrationRepairTone(tone: IntegrationRepairTone): BadgeTone {
  return badgeTones[tone];
}

/** 表示しないときは null。 */
export function integrationRepairDisplay(
  view: IntegrationRepairView | null | undefined,
): IntegrationRepairDisplay | null {
  if (!view) return null;
  const rows: IntegrationRepairRow[] = [
    { label: "状態", value: stateLabels[view.state] },
    { label: "試行", value: `${view.attempt} / ${view.max_attempts}` },
    { label: "target", value: view.target_ref ? `${view.target_ref} @ ${view.target_sha}` : view.target_sha },
  ];
  if (view.before_sha) rows.push({ label: "修復前", value: view.before_sha });
  if (view.conflict_files && view.conflict_files.length > 0) {
    rows.push({ label: "衝突ファイル", value: view.conflict_files.join(", ") });
  }
  if (view.reason) rows.push({ label: "打ち切り理由", value: `${reasonLabels[view.reason]}（${view.reason}）` });
  if (view.rollback_to_sha) rows.push({ label: "戻し先", value: view.rollback_to_sha });
  if (view.fallback != null) {
    rows.push({ label: "従来経路へ", value: view.fallback ? "はい（従来の配送経路へ戻す）" : "いいえ" });
  }
  return {
    heading: INTEGRATION_REPAIR_HEADING,
    label: INTEGRATION_REPAIR_LABEL,
    state: view.state,
    stateLabel: stateLabels[view.state],
    tone: tones[view.state],
    rows,
  };
}

/** 受信箱の注意項目のうち、Failed に付いた integration repair だけを取り出す。 */
export function attentionIntegrationRepair(item: AttentionItem): IntegrationRepairView | null {
  return item.type === "failed" ? (item.integration_repair ?? null) : null;
}
