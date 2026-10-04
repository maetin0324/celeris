import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AttentionItem, IntegrationRepairView, TaskRef } from "~/celeris/types";
import {
  attentionIntegrationRepair,
  INTEGRATION_REPAIR_HEADING,
  INTEGRATION_REPAIR_LABEL,
  IntegrationRepairPanel,
  integrationRepairLines,
  integrationRepairTone,
} from "~/components/IntegrationRepairPanel";

// celeris ADR-0120 D5: TaskDetail.integration_repair / AttentionItem::Failed.integration_repair の表示。
const scheduled: IntegrationRepairView = {
  state: "scheduled",
  attempt: 1,
  max_attempts: 2,
  target_ref: "main",
  target_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  before_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  conflict_files: ["crates/a.rs", "gui/b.tsx"],
  work_unit_id: "01WU",
};

const exhausted: IntegrationRepairView = {
  ...scheduled,
  state: "exhausted",
  attempt: 2,
  reason: "limit_reached",
  rollback_to_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  fallback: true,
};

describe("integration repair", () => {
  it("describes the scheduled state with attempt, target and conflicts", () => {
    const lines = integrationRepairLines(scheduled);
    expect(lines[0]).toBe("状態: 修復中（成果を保ったまま衝突を解消しています）（1/2 回目）");
    expect(lines).toContain("target: main @ aaaaaaaaaaaa");
    expect(lines).toContain("修復前の HEAD: bbbbbbbbbbbb");
    expect(lines).toContain("衝突ファイル: crates/a.rs, gui/b.tsx");
    expect(lines.some((l) => l.startsWith("打ち切り理由"))).toBe(false);
  });

  it("describes the resolved state", () => {
    const lines = integrationRepairLines({ ...scheduled, state: "resolved", target_ref: null });
    expect(lines[0]).toBe("状態: 解消済み（最新の target で review を再開）（1/2 回目）");
    expect(lines).toContain("target: aaaaaaaaaaaa");
  });

  it("describes the exhausted state with reason, rollback and fallback", () => {
    const lines = integrationRepairLines(exhausted);
    expect(lines[0]).toBe("状態: 打ち切り（従来の経路へ戻しました）（2/2 回目）");
    expect(lines).toContain("打ち切り理由: 上限に達した（limit_reached）");
    expect(lines).toContain("rollback 先: bbbbbbbbbbbb");
    expect(lines).toContain("fallback: 未同期の HEAD で review へ進めました");
  });

  it("uses a label and tone distinct from implementation failure", () => {
    for (const state of ["scheduled", "resolved", "exhausted"] as const) {
      expect(integrationRepairTone({ ...scheduled, state })).not.toBe("danger");
    }
    const html = renderToStaticMarkup(<IntegrationRepairPanel repair={exhausted} />);
    expect(html).toContain(INTEGRATION_REPAIR_HEADING);
    expect(html).toContain(INTEGRATION_REPAIR_LABEL);
    expect(html).toContain('data-integration-repair-state="exhausted"');
    expect(html).not.toContain("失敗: ");
    expect(html).not.toContain("bg-danger-soft");
    expect(html).toContain("bg-warning-soft");
  });

  it("renders nothing when integration_repair is null or missing", () => {
    expect(renderToStaticMarkup(<IntegrationRepairPanel repair={null} />)).toBe("");
    expect(renderToStaticMarkup(<IntegrationRepairPanel repair={undefined} />)).toBe("");
  });

  it("shows the repair on a failed inbox item only", () => {
    const task = { id: "01T", title: "t" } as TaskRef;
    const failed = {
      type: "failed",
      task,
      reason: "boom",
      class: "work",
      integration_repair: scheduled,
    } as AttentionItem;
    expect(attentionIntegrationRepair(failed)).toEqual(scheduled);
    const html = renderToStaticMarkup(<IntegrationRepairPanel repair={attentionIntegrationRepair(failed)} compact />);
    expect(html).toContain(INTEGRATION_REPAIR_HEADING);
    expect(html).toContain("bg-info-soft");
    const plain = { type: "failed", task, reason: "boom", class: "work" } as AttentionItem;
    expect(attentionIntegrationRepair(plain)).toBeNull();
    const nulled = { ...failed, integration_repair: null } as AttentionItem;
    expect(attentionIntegrationRepair(nulled)).toBeNull();
    const other = { type: "requeue_limit_near", task, count: 1, max: 3 } as AttentionItem;
    expect(attentionIntegrationRepair(other)).toBeNull();
  });
});
