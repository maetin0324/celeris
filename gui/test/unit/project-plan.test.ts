import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { decideProjectPlan, startProjectPlan } from "~/celeris/projects-admin.server";
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
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

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

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => {
  await mock.close();
});

describe("decideProjectPlan (POST /projects/{id}/project-plan/{version}/decide)", () => {
  it("approve: version をパスに、decision だけを本文に送る", async () => {
    mock.on("POST", "/api/v1/projects/p1/project-plan/2/decide", (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ decision: "approve" });
      sendJson(res, 202, { decision: "approve", milestones: ["m1"], plan_task_id: "t-plan", tasks: ["t1"] });
    });
    const form = new FormData();
    form.set("version", "2");
    form.set("decision", "approve");
    const result = await decideProjectPlan(client, "p1", form);
    expect(result).toEqual({
      ok: true,
      op: "project_plan_decide",
      decided: { decision: "approve", milestones: ["m1"], plan_task_id: "t-plan", tasks: ["t1"] },
    });
  });

  it("reject: note を送り、422 / 409 はそのまま ActionError にする", async () => {
    mock.on("POST", "/api/v1/projects/p1/project-plan/1/decide", (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ decision: "reject", note: "切り方が違う" });
      sendProblem(res, { status: 409, code: "project_plan_already_decided", detail: "already decided" });
    });
    const form = new FormData();
    form.set("version", "1");
    form.set("decision", "reject");
    form.set("note", "切り方が違う");
    const result = await decideProjectPlan(client, "p1", form);
    expect(result).toMatchObject({
      ok: false,
      op: "project_plan_decide",
      error: { status: 409, code: "project_plan_already_decided" },
    });
  });
});

describe("startProjectPlan の mode（案件計画 / 計画の見直し）", () => {
  it("mode=milestones を送る（replan になるかは celeris が決める）", async () => {
    mock.on("POST", "/api/v1/projects/p1/plan", (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ mode: "milestones", note: "PoC を分けたい" });
      sendJson(res, 202, { task_id: "01JREPLAN" });
    });
    const form = new FormData();
    form.set("mode", "milestones");
    form.set("note", "PoC を分けたい");
    const result = await startProjectPlan(client, "p1", form);
    expect(result).toEqual({ ok: true, op: "project_plan", accepted: { task_id: "01JREPLAN" } });
  });
});
