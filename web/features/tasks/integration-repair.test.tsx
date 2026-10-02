import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AttentionItem, IntegrationRepairView } from "../../api/generated/types";
import {
  attentionIntegrationRepair,
  INTEGRATION_REPAIR_HEADING,
  INTEGRATION_REPAIR_LABEL,
  integrationRepairDisplay,
} from "./integration-repair";
import { IntegrationRepairPanel } from "./integration-repair-panel";

const scheduled: IntegrationRepairView = {
  state: "scheduled",
  attempt: 1,
  max_attempts: 2,
  target_ref: "main",
  target_sha: "aaaa1111",
  before_sha: "bbbb2222",
  conflict_files: ["crates/a.rs", "web/b.ts"],
};

const task = { id: "T1", title: "t", status: "failed" } as AttentionItem extends { task: infer R } ? R : never;

describe("integration repair", () => {
  it("scheduled の文言（試行・target・修復前・衝突ファイル）", () => {
    const display = integrationRepairDisplay(scheduled);
    expect(display?.heading).toBe("target drift に伴う integration repair");
    expect(display?.stateLabel).toContain("scheduled");
    const rows = Object.fromEntries(display?.rows.map((r) => [r.label, r.value]) ?? []);
    expect(rows.試行).toBe("1 / 2");
    expect(rows.target).toBe("main @ aaaa1111");
    expect(rows.修復前).toBe("bbbb2222");
    expect(rows.衝突ファイル).toBe("crates/a.rs, web/b.ts");
    expect(rows.打ち切り理由).toBeUndefined();
  });

  it("resolved の文言", () => {
    const display = integrationRepairDisplay({ ...scheduled, state: "resolved", conflict_files: [] });
    expect(display?.stateLabel).toContain("resolved");
    expect(display?.tone).toBe("success");
    expect(display?.rows.find((r) => r.label === "衝突ファイル")).toBeUndefined();
  });

  it("exhausted の文言（reason・rollback_to_sha・fallback）", () => {
    const display = integrationRepairDisplay({
      ...scheduled,
      state: "exhausted",
      attempt: 2,
      reason: "limit_reached",
      rollback_to_sha: "cccc3333",
      fallback: true,
    });
    const rows = Object.fromEntries(display?.rows.map((r) => [r.label, r.value]) ?? []);
    expect(display?.stateLabel).toContain("exhausted");
    expect(rows.打ち切り理由).toContain("limit_reached");
    expect(rows.戻し先).toBe("cccc3333");
    expect(rows.従来経路へ).toContain("はい");
  });

  it("実装失敗とは別のラベル・色で出す", () => {
    const html = renderToStaticMarkup(<IntegrationRepairPanel view={scheduled} />);
    expect(html).toContain(INTEGRATION_REPAIR_HEADING);
    expect(html).toContain(INTEGRATION_REPAIR_LABEL);
    expect(INTEGRATION_REPAIR_LABEL).toContain("実装失敗ではない");
    expect(html).not.toContain("失敗</h2>");
    expect(html).not.toMatch(/bg-red|border-red/);
    expect(html).not.toContain('role="alert"');
  });

  it("null・欠落なら何も出さない", () => {
    expect(integrationRepairDisplay(null)).toBeNull();
    expect(integrationRepairDisplay(undefined)).toBeNull();
    expect(renderToStaticMarkup(<IntegrationRepairPanel view={null} />)).toBe("");
    expect(renderToStaticMarkup(<IntegrationRepairPanel view={undefined} />)).toBe("");
  });

  it("inbox の Failed 項目で表示し、他の項目・欠落では出さない", () => {
    const failed: AttentionItem = {
      type: "failed",
      at: "2026-10-02T00:00:00Z",
      class: "review_rejected" as never,
      reason: "x",
      task,
      integration_repair: scheduled,
    };
    expect(attentionIntegrationRepair(failed)).toEqual(scheduled);
    const html = renderToStaticMarkup(<IntegrationRepairPanel view={attentionIntegrationRepair(failed)} />);
    expect(html).toContain(INTEGRATION_REPAIR_HEADING);
    const { integration_repair: _omit, ...plain } = failed as Extract<AttentionItem, { type: "failed" }>;
    expect(attentionIntegrationRepair(plain)).toBeNull();
    const other: AttentionItem = { type: "requeue_limit_near", at: "t", count: 1, max: 3, task };
    expect(attentionIntegrationRepair(other)).toBeNull();
  });
});
