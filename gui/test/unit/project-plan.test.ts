// celeris ADR-0079 D13（Phase R5a）: 案件計画の書き込み（`POST /projects/{id}/plan`・`…/decide`）の中継は外した
// （celeris が 410）。残るのは凍結した DAG を読むための表示の純粋関数だけ。
import { describe, expect, it } from "vitest";
import type { PlanDagNode } from "~/celeris/types";
import {
  planChangeLabel,
  planLayers,
  planProgressText,
  planQuotaText,
  planStopReasonLabel,
  planTopologicalOrder,
  projectPlanDecisionValid,
} from "~/lib/project-plan";

const node = (key: string, depends_on: string[] = [], over: Partial<PlanDagNode> = {}): PlanDagNode => ({
  key,
  title: `title-${key}`,
  depends_on,
  milestone_id: `m-${key}`,
  task_id: `t-${key}`,
  work_units_done: 0,
  work_units_total: 0,
  children_done: 0,
  children_total: 0,
  quota: [],
  ...over,
});

/** ADR-0074 D3.5（Phase F4b (h)）: 案件ページの DAG の並べ方と文言（判断は celeris が返した値のまま）。 */
describe("project plan DAG helpers", () => {
  it("依存の最長経路で層に分け、層の中は celeris の並びのまま", () => {
    const nodes = [node("paper", ["poc", "survey"]), node("survey"), node("poc", ["survey"]), node("ops")];
    const layers = planLayers(nodes).map((l) => l.map((n) => n.key));
    expect(layers).toEqual([["survey", "ops"], ["poc"], ["paper"]]);
    // モバイル幅の縦の一覧はトポロジカル順。
    expect(planTopologicalOrder(nodes).map((n) => n.key)).toEqual(["survey", "ops", "poc", "paper"]);
  });

  it("知らない key への依存は無視し、循環でも落ちない", () => {
    expect(planLayers([node("a", ["zzz"])]).map((l) => l.map((n) => n.key))).toEqual([["a"]]);
    const cyclic = planLayers([node("a", ["b"]), node("b", ["a"])]);
    expect(
      cyclic
        .flat()
        .map((n) => n.key)
        .sort(),
    ).toEqual(["a", "b"]);
  });

  it("進み具合・quota・止まっている理由・変更の文言", () => {
    expect(planProgressText(node("a"))).toBeNull();
    expect(
      planProgressText(node("a", [], { work_units_done: 1, work_units_total: 3, children_done: 0, children_total: 2 })),
    ).toBe("WU 1/3・子 0/2");
    expect(planQuotaText([])).toBeNull();
    expect(
      planQuotaText([
        { source: "claude", window: "five_hour", used_pct: 4.5, runs: 2, method_counts: {} },
        { source: "claude", window: "seven_day", used_pct: null, runs: 1, method_counts: {} },
      ]),
    ).toBe("claude 5h 4.5%");
    expect(planStopReasonLabel("awaiting_go")).toBe("前の途中目標の判定待ち");
    expect(planStopReasonLabel("awaiting_human")).toBe("途中確認待ち");
    expect(planChangeLabel("cancel")).toBe("取り下げ");
  });

  it("却下には理由が要る（承認は一言無しでよい）", () => {
    expect(projectPlanDecisionValid("approve", "")).toBe(true);
    expect(projectPlanDecisionValid("reject", "  ")).toBe(false);
    expect(projectPlanDecisionValid("reject", "切り方が違う")).toBe(true);
  });
});
