// celeris ADR-0120 D5: review 前同期で target が進み衝突したときの integration repair の状況。
// 実装失敗（WorkerFinished の失敗・レビュー不合格。`FailureBanner` の赤）とは別物なので、
// 色（scheduled=info / resolved=success / exhausted=warning。danger は使わない）とラベルを分ける。
// 表示文の組み立ては純関数（`integrationRepairLines`）にして試験しやすくする。
import type { AttentionItem, IntegrationRepairExhaustReason, IntegrationRepairView } from "~/celeris/types";
import { Alert } from "~/components/ui/misc";
import type { Tone } from "~/components/ui/tone";

export const INTEGRATION_REPAIR_HEADING = "target drift に伴う integration repair";

/** 実装失敗（「失敗: …」）と取り違えないためのラベル。 */
export const INTEGRATION_REPAIR_LABEL = "統合修復（実装失敗ではない）";

const STATE_TEXT: Record<IntegrationRepairView["state"], string> = {
  scheduled: "修復中（成果を保ったまま衝突を解消しています）",
  resolved: "解消済み（最新の target で review を再開）",
  exhausted: "打ち切り（従来の経路へ戻しました）",
};

const STATE_TONE: Record<IntegrationRepairView["state"], Tone> = {
  scheduled: "info",
  resolved: "success",
  exhausted: "warning",
};

const REASON_TEXT: Record<IntegrationRepairExhaustReason, string> = {
  limit_reached: "上限に達した",
  plan_issue: "修復 WU が plan_issue で終えた",
  work_unit_failed: "修復 WU が failed になった",
  budget_exhausted: "修復 WU が予算切れのまま再開できない",
  result_untrusted: "成果の保持が確認できない",
  abort_failed: "rebase --abort が失敗した",
  worktree_unavailable: "worktree が使えない",
};

export function integrationRepairTone(view: IntegrationRepairView): Tone {
  return STATE_TONE[view.state];
}

function short(sha: string | null | undefined): string {
  return sha ? sha.slice(0, 12) : "不明";
}

/** パネルに出す行（見出し・ラベルは除く）。`null` / 欠落なら呼ばない（`IntegrationRepairPanel` が何も出さない）。 */
export function integrationRepairLines(view: IntegrationRepairView): string[] {
  const lines = [`状態: ${STATE_TEXT[view.state]}（${view.attempt}/${view.max_attempts} 回目）`];
  lines.push(`target: ${view.target_ref ? `${view.target_ref} @ ` : ""}${short(view.target_sha)}`);
  if (view.before_sha) lines.push(`修復前の HEAD: ${short(view.before_sha)}`);
  const files = view.conflict_files ?? [];
  if (files.length > 0) lines.push(`衝突ファイル: ${files.join(", ")}`);
  if (view.reason) lines.push(`打ち切り理由: ${REASON_TEXT[view.reason] ?? view.reason}（${view.reason}）`);
  if (view.rollback_to_sha) lines.push(`rollback 先: ${short(view.rollback_to_sha)}`);
  if (view.fallback === true) lines.push("fallback: 未同期の HEAD で review へ進めました");
  else if (view.fallback === false) lines.push("fallback: なし");
  return lines;
}

/** 受信箱の項目から integration repair を取り出す（`failed` 以外・欠落・null は `null`）。 */
export function attentionIntegrationRepair(item: AttentionItem): IntegrationRepairView | null {
  if (item.type !== "failed") return null;
  return item.integration_repair ?? null;
}

export function IntegrationRepairPanel({
  repair,
  compact = false,
}: {
  repair: IntegrationRepairView | null | undefined;
  compact?: boolean;
}) {
  if (!repair) return null;
  return (
    <section
      aria-label={INTEGRATION_REPAIR_HEADING}
      data-testid="integration-repair-panel"
      data-integration-repair-state={repair.state}
      className={compact ? "mt-3" : undefined}
    >
      <Alert tone={integrationRepairTone(repair)} icon="gitBranch" title={INTEGRATION_REPAIR_HEADING}>
        <p>
          <span
            className="inline-flex items-center rounded-full border border-current px-2 py-0.5 text-xs font-medium"
            data-testid="integration-repair-label"
          >
            {INTEGRATION_REPAIR_LABEL}
          </span>
        </p>
        <ul className="mt-1 space-y-0.5 text-sm text-fg" data-testid="integration-repair-lines">
          {integrationRepairLines(repair).map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      </Alert>
    </section>
  );
}
